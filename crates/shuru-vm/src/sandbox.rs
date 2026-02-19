use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use crossbeam_channel::Receiver;

use shuru_darwin::*;

use crate::proto::{ExecRequest, ExecResponse};
use crate::VSOCK_PORT;

pub struct VmConfigBuilder {
    kernel: Option<String>,
    rootfs: Option<String>,
    initrd: Option<String>,
    cpus: usize,
    memory_mb: u64,
}

impl VmConfigBuilder {
    pub(crate) fn new() -> Self {
        VmConfigBuilder {
            kernel: None,
            rootfs: None,
            initrd: None,
            cpus: 2,
            memory_mb: 2048,
        }
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

        let serial_attachment = FileHandleSerialAttachment::new(
            std::io::stdin().as_raw_fd(),
            std::io::stdout().as_raw_fd(),
        );
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

    /// Connect to the guest via vsock with retry, run a command, and stream
    /// output to the provided writers. Returns the guest process exit code.
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
