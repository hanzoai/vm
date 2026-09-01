//! VirtualMachine driving a `cloud-hypervisor` child process over its API
//! socket: `vm.create` + `vm.boot` on start, `vm.shutdown` + `vmm.shutdown`
//! on stop. One `virtiofsd` child serves each shared directory, and the
//! in-process vhost-user-net backend serves the proxy socketpair, so guest
//! memory is always mapped `shared=on`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::api;
use crate::configuration::{ConfigData, VirtualMachineConfiguration};
use crate::error::{Result, VzError};
use crate::net_backend;

const GUEST_CID: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Stopped = 0,
    Running = 1,
    Error = 3,
    // Paused/Starting/etc. kept as discriminants for Darwin compatibility.
    Unknown = -1,
}

pub struct VirtualMachine {
    config: ConfigData,
    run_dir: PathBuf,
    api_socket: String,
    vsock_socket: String,
    ch_pid: Mutex<Option<i32>>,
    virtiofsd: Mutex<Vec<Child>>,
    state_tx: Sender<VmState>,
    state_rx: Receiver<VmState>,
    running: Arc<AtomicBool>,
}

/// Locate a helper binary on PATH or in `~/.local/bin`.
fn find_binary(name: &str) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let candidate = Path::new(&home).join(".local/bin").join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Duplicate a raw fd into an owned handle for child stdio.
fn dup_stdio(fd: i32) -> Result<Stdio> {
    let duped = unsafe { libc::dup(fd) };
    if duped < 0 {
        return Err(VzError::new(format!(
            "dup({}) failed: {}",
            fd,
            std::io::Error::last_os_error()
        )));
    }
    Ok(Stdio::from(unsafe { OwnedFd::from_raw_fd(duped) }))
}

fn wait_for_socket(path: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Path::new(path).exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

impl VirtualMachine {
    pub fn new(config: &VirtualMachineConfiguration) -> Self {
        let inner = config.inner.borrow().clone();
        let (state_tx, state_rx) = bounded(1);

        // Sockets live next to the disk (the per-instance directory), or in
        // a private temp directory when there is no disk.
        let run_dir = inner
            .disk_path
            .as_deref()
            .and_then(|p| Path::new(p).parent())
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("hanzo-vm-{}", std::process::id()))
            });
        let _ = std::fs::create_dir_all(&run_dir);

        let api_socket = run_dir.join("ch-api.sock").to_string_lossy().into_owned();
        let vsock_socket = run_dir.join("vsock.sock").to_string_lossy().into_owned();

        VirtualMachine {
            config: inner,
            run_dir,
            api_socket,
            vsock_socket,
            ch_pid: Mutex::new(None),
            virtiofsd: Mutex::new(Vec::new()),
            state_tx,
            state_rx,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn supported() -> bool {
        Path::new("/dev/kvm").exists() && find_binary("cloud-hypervisor").is_some()
    }

    pub fn start(&self) -> Result<()> {
        let ch_bin = find_binary("cloud-hypervisor").ok_or_else(|| {
            VzError::new("cloud-hypervisor not found on PATH or in ~/.local/bin")
        })?;

        // One virtiofsd child per shared directory; the socket must be
        // listening before cloud-hypervisor creates the device.
        let mut fs_sockets = Vec::new();
        if !self.config.mounts.is_empty() {
            let fsd_bin = find_binary("virtiofsd").ok_or_else(|| {
                VzError::new("virtiofsd not found on PATH or in ~/.local/bin")
            })?;
            for (tag, host_path, _read_only) in &self.config.mounts {
                let socket = self.run_dir.join(format!("fs-{}.sock", tag));
                let log = std::fs::File::create(self.run_dir.join(format!("fs-{}.log", tag)))
                    .map_err(|e| VzError::new(format!("virtiofsd log: {}", e)))?;
                let child = Command::new(&fsd_bin)
                    .arg(format!("--socket-path={}", socket.display()))
                    .arg("--shared-dir")
                    .arg(host_path)
                    .arg("--cache")
                    .arg("auto")
                    .arg("--sandbox")
                    .arg("none")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::from(log))
                    .spawn()
                    .map_err(|e| VzError::new(format!("spawn virtiofsd: {}", e)))?;
                self.virtiofsd.lock().unwrap().push(child);
                fs_sockets.push((tag.clone(), socket));
            }
            for (tag, socket) in &fs_sockets {
                if !wait_for_socket(&socket.to_string_lossy(), Duration::from_secs(5)) {
                    return Err(VzError::new(format!(
                        "virtiofsd socket for {} did not appear",
                        tag
                    )));
                }
            }
        }

        // In-process vhost-user-net backend over the proxy socketpair.
        let net_socket = self.run_dir.join("net.sock");
        if let Some(fd) = self.config.network_fd {
            net_backend::spawn(&net_socket.to_string_lossy(), fd)?;
        }

        // Serial console: the virtio-console is wired to the child's stdio.
        let stdin = match self.config.serial_read_fd {
            Some(fd) => dup_stdio(fd)?,
            None => Stdio::null(),
        };
        let stdout = match self.config.serial_write_fd {
            Some(fd) => dup_stdio(fd)?,
            None => Stdio::null(),
        };

        let _ = std::fs::remove_file(&self.api_socket);
        let _ = std::fs::remove_file(&self.vsock_socket);

        let mut child = Command::new(&ch_bin)
            .arg("--api-socket")
            .arg(format!("path={}", self.api_socket))
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| VzError::new(format!("spawn cloud-hypervisor: {}", e)))?;

        if !wait_for_socket(&self.api_socket, Duration::from_secs(5)) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(VzError::new("cloud-hypervisor API socket did not appear"));
        }

        let vm_config = self.vm_config_json(&fs_sockets, &net_socket);
        if let Err(e) = api::put(&self.api_socket, "vm.create", Some(&vm_config.to_string()))
            .and_then(|_| api::put(&self.api_socket, "vm.boot", None))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }

        *self.ch_pid.lock().unwrap() = Some(child.id() as i32);
        self.running.store(true, Ordering::Release);
        let _ = self.state_tx.try_send(VmState::Running);

        // Monitor thread: the cloud-hypervisor process exits when the guest
        // shuts down (or on vmm.shutdown / kill from stop()).
        let running = self.running.clone();
        let state_tx = self.state_tx.clone();
        std::thread::Builder::new()
            .name("hanzo-vm-monitor".into())
            .spawn(move || {
                let _ = child.wait();
                running.store(false, Ordering::Release);
                let _ = state_tx.try_send(VmState::Stopped);
            })
            .map_err(|e| VzError::new(format!("spawn monitor thread: {}", e)))?;

        Ok(())
    }

    fn vm_config_json(
        &self,
        fs_sockets: &[(String, PathBuf)],
        net_socket: &Path,
    ) -> serde_json::Value {
        let c = &self.config;
        let mut cfg = serde_json::json!({
            "cpus": { "boot_vcpus": c.cpu_count, "max_vcpus": c.cpu_count },
            // vhost-user devices (fs, net) require shared guest memory.
            "memory": { "size": c.memory_size, "shared": true },
            "payload": { "kernel": c.kernel_path, "cmdline": c.command_line },
            "serial": { "mode": "Off" },
            "console": { "mode": "Tty" },
            "rng": { "src": "/dev/urandom" },
        });
        if let Some(ref initrd) = c.initrd_path {
            cfg["payload"]["initramfs"] = serde_json::json!(initrd);
        }
        if let Some(ref disk) = c.disk_path {
            cfg["disks"] = serde_json::json!([{ "path": disk, "readonly": c.disk_read_only }]);
        }
        if c.has_socket {
            cfg["vsock"] = serde_json::json!({ "cid": GUEST_CID, "socket": self.vsock_socket });
        }
        if !fs_sockets.is_empty() {
            let fs: Vec<_> = fs_sockets
                .iter()
                .map(|(tag, socket)| {
                    serde_json::json!({
                        "tag": tag,
                        "socket": socket.to_string_lossy(),
                        "num_queues": 1,
                        "queue_size": 1024,
                    })
                })
                .collect();
            cfg["fs"] = serde_json::json!(fs);
        }
        if c.network_fd.is_some() {
            let mut net = serde_json::json!({
                "vhost_user": true,
                "vhost_socket": net_socket.to_string_lossy(),
                "num_queues": 2,
                "queue_size": 256,
            });
            if let Some(mac) = c.network_mac {
                net["mac"] = serde_json::json!(format!(
                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                ));
            }
            cfg["net"] = serde_json::json!([net]);
        }
        cfg
    }

    pub fn stop(&self) -> Result<()> {
        // Ask the VMM to shut down; fall back to SIGKILL.
        let _ = api::put(&self.api_socket, "vm.shutdown", None);
        let _ = api::put(&self.api_socket, "vmm.shutdown", None);

        if let Some(pid) = *self.ch_pid.lock().unwrap() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.running.load(Ordering::Acquire) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if self.running.load(Ordering::Acquire) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }

        for mut child in self.virtiofsd.lock().unwrap().drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }

        self.running.store(false, Ordering::Release);
        let _ = self.state_tx.try_send(VmState::Stopped);
        Ok(())
    }

    pub fn state_channel(&self) -> Receiver<VmState> {
        self.state_rx.clone()
    }

    pub fn can_start(&self) -> bool {
        !self.running.load(Ordering::Acquire)
    }

    pub fn can_stop(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn can_pause(&self) -> bool {
        false
    }
    pub fn can_resume(&self) -> bool {
        false
    }

    pub fn can_request_stop(&self) -> bool {
        self.can_stop()
    }

    /// Connect to a vsock port on the guest through cloud-hypervisor's
    /// hybrid vsock socket: send `CONNECT <port>\n`, expect `OK <n>\n`,
    /// then the stream is a raw pipe to the guest listener.
    ///
    /// The connected `UnixStream` fd is rewrapped as a `TcpStream` because
    /// the platform-neutral sandbox API is written against `TcpStream`;
    /// both are plain stream sockets, so read/write/shutdown/try_clone all
    /// behave (`set_nodelay` fails and is ignored by callers).
    pub fn connect_to_vsock_port(&self, port: u32) -> Result<TcpStream> {
        let mut stream = UnixStream::connect(&self.vsock_socket)
            .map_err(|e| VzError::new(format!("vsock connect failed: {}", e)))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

        stream
            .write_all(format!("CONNECT {}\n", port).as_bytes())
            .map_err(|e| VzError::new(format!("vsock handshake send: {}", e)))?;

        // Read the response a byte at a time so no guest data is consumed.
        let mut line = Vec::with_capacity(16);
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(1) => {
                    if byte[0] == b'\n' {
                        break;
                    }
                    line.push(byte[0]);
                    if line.len() > 32 {
                        return Err(VzError::new("vsock handshake: oversized response"));
                    }
                }
                Ok(_) => return Err(VzError::new("vsock connect refused (EOF)")),
                Err(e) => return Err(VzError::new(format!("vsock handshake read: {}", e))),
            }
        }
        if !line.starts_with(b"OK ") {
            return Err(VzError::new(format!(
                "vsock connect refused: {}",
                String::from_utf8_lossy(&line)
            )));
        }

        let _ = stream.set_read_timeout(None);
        Ok(unsafe { TcpStream::from_raw_fd(stream.into_raw_fd()) })
    }

    pub fn state(&self) -> VmState {
        if self.running.load(Ordering::Acquire) {
            VmState::Running
        } else {
            VmState::Stopped
        }
    }
}

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        // Child processes outlive a dropped VirtualMachine unless killed.
        if let Some(pid) = *self.ch_pid.lock().unwrap() {
            if self.running.load(Ordering::Acquire) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        for mut child in self.virtiofsd.lock().unwrap().drain(..) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
