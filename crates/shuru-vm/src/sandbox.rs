use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossbeam_channel::Receiver;

use shuru_darwin::terminal;
use shuru_darwin::*;

use crate::proto::{ControlMessage, ExecRequest, ExecResponse};
use crate::VSOCK_PORT;

// --- VmConfigBuilder ---

pub struct VmConfigBuilder {
    kernel: Option<String>,
    rootfs: Option<String>,
    initrd: Option<String>,
    cpus: usize,
    memory_mb: u64,
    console: bool,
}

impl VmConfigBuilder {
    pub(crate) fn new() -> Self {
        VmConfigBuilder {
            kernel: None,
            rootfs: None,
            initrd: None,
            cpus: 2,
            memory_mb: 2048,
            console: true,
        }
    }

    /// When false, serial console stdin is disconnected and stdout goes to
    /// stderr. This prevents the serial console from consuming host stdin
    /// in exec/shell mode.
    pub fn console(mut self, enabled: bool) -> Self {
        self.console = enabled;
        self
    }

    pub fn kernel(mut self, path: impl Into<String>) -> Self {
        self.kernel = Some(path.into());
        self
    }

    pub fn rootfs(mut self, path: impl Into<String>) -> Self {
        self.rootfs = Some(path.into());
        self
    }

    pub fn initrd(mut self, path: impl Into<String>) -> Self {
        self.initrd = Some(path.into());
        self
    }

    pub fn cpus(mut self, n: usize) -> Self {
        self.cpus = n;
        self
    }

    pub fn memory_mb(mut self, mb: u64) -> Self {
        self.memory_mb = mb;
        self
    }

    pub fn build(self) -> Result<Sandbox> {
        let kernel_path = self.kernel.context("kernel path is required")?;
        let rootfs_path = self.rootfs.context("rootfs path is required")?;

        if !VirtualMachine::supported() {
            bail!("Virtualization is not supported on this machine");
        }

        let boot_loader = LinuxBootLoader::new_with_kernel(&kernel_path);
        if let Some(ref initrd) = self.initrd {
            boot_loader.set_initrd(initrd);
        }
        boot_loader.set_command_line("console=hvc0 root=/dev/vda rw");

        let memory_bytes = self.memory_mb * 1024 * 1024;
        let config = VirtualMachineConfiguration::new(&boot_loader, self.cpus, memory_bytes);

        let serial_attachment = if self.console {
            FileHandleSerialAttachment::new(
                std::io::stdin().as_raw_fd(),
                std::io::stdout().as_raw_fd(),
            )
        } else {
            FileHandleSerialAttachment::new_write_only(std::io::stderr().as_raw_fd())
        };
        let serial = VirtioConsoleSerialPort::new_with_attachment(&serial_attachment);
        config.set_serial_ports(&[serial]);

        let disk_attachment = DiskImageAttachment::new(&rootfs_path, false)
            .map_err(|e| anyhow::anyhow!("Failed to create disk attachment: {}", e))?;
        let block_device = VirtioBlockDevice::new(&disk_attachment);
        config.set_storage_devices(&[block_device]);

        let net_attachment = NATNetworkAttachment::new();
        let net_device = VirtioNetworkDevice::new_with_attachment(&net_attachment);
        net_device.set_mac_address(&MACAddress::random_local());
        config.set_network_devices(&[net_device]);

        let socket_device = VirtioSocketDevice::new();
        config.set_socket_devices(&[socket_device]);

        config.set_entropy_devices(&[VirtioEntropyDevice::new()]);
        config.set_memory_balloon_devices(&[VirtioMemoryBalloonDevice::new()]);

        config
            .validate()
            .map_err(|e| anyhow::anyhow!("VM configuration invalid: {}", e))?;

        Ok(Sandbox {
            vm: VirtualMachine::new(&config),
        })
    }
}

// --- Sandbox ---

pub struct Sandbox {
    vm: VirtualMachine,
}

impl Sandbox {
    pub fn builder() -> VmConfigBuilder {
        VmConfigBuilder::new()
    }

    pub fn start(&self) -> Result<()> {
        self.vm
            .start()
            .map_err(|e| anyhow::anyhow!("Failed to start VM: {}", e))
    }

    pub fn stop(&self) -> Result<()> {
        self.vm
            .stop()
            .map_err(|e| anyhow::anyhow!("Failed to stop VM: {}", e))
    }

    pub fn state_channel(&self) -> Receiver<VmState> {
        self.vm.state_channel()
    }

    /// Run a command non-interactively over vsock, streaming output to the
    /// provided writers. Returns the guest process exit code.
    pub fn exec(
        &self,
        argv: &[impl AsRef<str>],
        stdout: &mut impl Write,
        stderr: &mut impl Write,
    ) -> Result<i32> {
        let stream = self.connect_vsock()?;

        let mut writer = stream.try_clone()?;
        let reader = BufReader::new(stream);

        let req = ExecRequest {
            argv: argv.iter().map(|s| s.as_ref().to_string()).collect(),
            env: HashMap::new(),
            tty: None,
            rows: None,
            cols: None,
        };
        writeln!(writer, "{}", serde_json::to_string(&req)?)?;
        writer.flush()?;

        let mut exit_code = 0;

        for line in reader.lines() {
            let line = line.context("reading vsock response")?;
            if line.is_empty() {
                continue;
            }

            let resp: ExecResponse =
                serde_json::from_str(&line).context("parsing vsock response")?;

            match resp.msg_type.as_str() {
                "stdout" => {
                    if let Some(data) = &resp.data {
                        write!(stdout, "{}", data)?;
                    }
                }
                "stderr" => {
                    if let Some(data) = &resp.data {
                        write!(stderr, "{}", data)?;
                    }
                }
                "exit" => {
                    exit_code = resp.code.unwrap_or(0);
                    break;
                }
                "error" => {
                    if let Some(data) = &resp.data {
                        write!(stderr, "guest error: {}", data)?;
                    }
                    exit_code = 1;
                    break;
                }
                _ => {}
            }
        }

        Ok(exit_code)
    }

    /// Run an interactive shell session with PTY support.
    /// Puts the host terminal in raw mode, relays I/O bidirectionally over
    /// vsock, and handles SIGWINCH for window resize.
    /// Returns the guest process exit code.
    pub fn shell(
        &self,
        argv: &[impl AsRef<str>],
        env: &HashMap<String, String>,
    ) -> Result<i32> {
        let stdin_fd = std::io::stdin().as_raw_fd();
        let (rows, cols) = terminal::terminal_size(stdin_fd);

        let stream = self.connect_vsock()?;

        // Send ExecRequest with tty=true
        let mut writer = stream.try_clone()?;
        let req = ExecRequest {
            argv: argv.iter().map(|s| s.as_ref().to_string()).collect(),
            env: env.clone(),
            tty: Some(true),
            rows: Some(rows),
            cols: Some(cols),
        };
        writeln!(writer, "{}", serde_json::to_string(&req)?)?;
        writer.flush()?;

        // Enter raw mode - TerminalState restores on drop
        let _raw_guard = terminal::TerminalState::enter_raw_mode(stdin_fd);

        // Install SIGWINCH handler
        terminal::install_sigwinch_handler();

        let done = Arc::new(AtomicBool::new(false));
        let exit_code = Arc::new(Mutex::new(0i32));

        // Thread A: stdin → vsock (send stdin data + resize messages)
        let done_a = done.clone();
        let mut vsock_writer = stream.try_clone()?;
        let stdin_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];

            while !done_a.load(Ordering::SeqCst) {
                if terminal::poll_read(stdin_fd, 100) {
                    let n = terminal::read_raw(stdin_fd, &mut buf);
                    if n == 0 {
                        break;
                    }
                    let data = String::from_utf8_lossy(&buf[..n]);
                    let msg = ControlMessage::Stdin {
                        data: data.into_owned(),
                    };
                    if writeln!(vsock_writer, "{}", serde_json::to_string(&msg).unwrap()).is_err() {
                        break;
                    }
                    let _ = vsock_writer.flush();
                }

                // Check SIGWINCH
                if terminal::sigwinch_received() {
                    let (rows, cols) = terminal::terminal_size(stdin_fd);
                    let msg = ControlMessage::Resize { rows, cols };
                    if writeln!(vsock_writer, "{}", serde_json::to_string(&msg).unwrap()).is_err() {
                        break;
                    }
                    let _ = vsock_writer.flush();
                }
            }
        });

        // Thread B: vsock → stdout (read responses, write output)
        let done_b = done.clone();
        let exit_code_b = exit_code.clone();
        let reader = BufReader::new(stream);
        let vsock_thread = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            for line in reader.lines() {
                let line = match line {
                    Ok(l) => l,
                    Err(_) => break,
                };
                if line.is_empty() {
                    continue;
                }

                let resp: ExecResponse = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                match resp.msg_type.as_str() {
                    "stdout" => {
                        if let Some(data) = &resp.data {
                            let _ = stdout.write_all(data.as_bytes());
                            let _ = stdout.flush();
                        }
                    }
                    "exit" => {
                        *exit_code_b.lock().unwrap() = resp.code.unwrap_or(0);
                        break;
                    }
                    "error" => {
                        if let Some(data) = &resp.data {
                            let _ = std::io::stderr()
                                .write_all(format!("guest error: {}\r\n", data).as_bytes());
                        }
                        *exit_code_b.lock().unwrap() = 1;
                        break;
                    }
                    _ => {}
                }
            }
            done_b.store(true, Ordering::SeqCst);
        });

        // Wait for threads
        let _ = vsock_thread.join();
        let _ = stdin_thread.join();

        // Restore SIGWINCH to default
        terminal::reset_sigwinch_handler();

        // Terminal restored by _raw_guard drop
        let code = *exit_code.lock().unwrap();
        Ok(code)
    }

    fn connect_vsock(&self) -> Result<TcpStream> {
        for attempt in 1..=10 {
            match self.vm.connect_to_vsock_port(VSOCK_PORT) {
                Ok(s) => return Ok(s),
                Err(e) => {
                    if attempt == 10 {
                        bail!("Failed to connect to guest after 10 attempts: {}", e);
                    }
                    tracing::debug!("vsock connect attempt {} failed: {}", attempt, e);
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        }
        unreachable!()
    }
}
