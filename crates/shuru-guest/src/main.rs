#[cfg(target_os = "linux")]
mod guest {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::io::FromRawFd;
    use std::process::{Command, Stdio};

    use serde::{Deserialize, Serialize};

    const VSOCK_PORT: u32 = 1024;

    #[derive(Deserialize)]
    pub struct ExecRequest {
        pub argv: Vec<String>,
        #[serde(default)]
        pub env: std::collections::HashMap<String, String>,
    }

    #[derive(Serialize)]
    pub struct ExecResponse {
        #[serde(rename = "type")]
        pub msg_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub data: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub code: Option<i32>,
    }

    fn mount_fs(source: &str, target: &str, fstype: &str) {
        use std::ffi::CString;

        let c_source = CString::new(source).unwrap();
        let c_target = CString::new(target).unwrap();
        let c_fstype = CString::new(fstype).unwrap();

        let ret = unsafe {
            libc::mount(
                c_source.as_ptr(),
                c_target.as_ptr(),
                c_fstype.as_ptr(),
                0,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            eprintln!(
                "shuru-guest: failed to mount {} on {}: {}",
                source,
                target,
                std::io::Error::last_os_error()
            );
        }
    }

    fn mount_filesystems() {
        mount_fs("proc", "/proc", "proc");
        mount_fs("sysfs", "/sys", "sysfs");
        mount_fs("devtmpfs", "/dev", "devtmpfs");
        std::fs::create_dir_all("/dev/pts").ok();
        mount_fs("devpts", "/dev/pts", "devpts");
        mount_fs("tmpfs", "/tmp", "tmpfs");
    }

    fn setup_networking() {
        unsafe {
            let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if sock < 0 {
                eprintln!("shuru-guest: failed to create socket for networking setup");
                return;
            }

            let mut ifr: libc::ifreq = std::mem::zeroed();
            let ifname = b"lo\0";
            std::ptr::copy_nonoverlapping(
                ifname.as_ptr(),
                ifr.ifr_name.as_mut_ptr() as *mut u8,
                3,
            );

            // Get current flags
            if libc::ioctl(sock, libc::SIOCGIFFLAGS as _, &mut ifr) < 0 {
                eprintln!("shuru-guest: failed to get lo flags");
                libc::close(sock);
                return;
            }

            // Set IFF_UP
            ifr.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short;
            if libc::ioctl(sock, libc::SIOCSIFFLAGS as _, &ifr) < 0 {
                eprintln!("shuru-guest: failed to bring up lo");
            }

            libc::close(sock);
        }
    }

    fn reap_zombies() {
        loop {
            let ret = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
            if ret <= 0 {
                break;
            }
        }
    }

    fn create_vsock_listener(port: u32) -> i32 {
        unsafe {
            let fd = libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM, 0);
            if fd < 0 {
                panic!(
                    "shuru-guest: failed to create vsock socket: {}",
                    std::io::Error::last_os_error()
                );
            }

            #[repr(C)]
            struct SockaddrVm {
                svm_family: libc::sa_family_t,
                svm_reserved1: u16,
                svm_port: u32,
                svm_cid: u32,
                svm_flags: u8,
                svm_zero: [u8; 3],
            }

            let addr = SockaddrVm {
                svm_family: libc::AF_VSOCK as libc::sa_family_t,
                svm_reserved1: 0,
                svm_port: port,
                svm_cid: libc::VMADDR_CID_ANY,
                svm_flags: 0,
                svm_zero: [0; 3],
            };

            let ret = libc::bind(
                fd,
                &addr as *const SockaddrVm as *const libc::sockaddr,
                std::mem::size_of::<SockaddrVm>() as libc::socklen_t,
            );
            if ret < 0 {
                panic!(
                    "shuru-guest: failed to bind vsock on port {}: {}",
                    port,
                    std::io::Error::last_os_error()
                );
            }

            let ret = libc::listen(fd, 1);
            if ret < 0 {
                panic!(
                    "shuru-guest: failed to listen on vsock: {}",
                    std::io::Error::last_os_error()
                );
            }

            fd
        }
    }

    fn handle_connection(fd: i32) {
        // SAFETY: fd is a valid socket from accept()
        let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
        let reader = BufReader::new(stream.try_clone().expect("failed to clone stream"));
        let mut writer = stream;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if line.is_empty() {
                continue;
            }

            let req: ExecRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let resp = ExecResponse {
                        msg_type: "error".into(),
                        data: Some(format!("invalid request: {}", e)),
                        code: None,
                    };
                    let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                    continue;
                }
            };

            if req.argv.is_empty() {
                let resp = ExecResponse {
                    msg_type: "error".into(),
                    data: Some("empty argv".into()),
                    code: None,
                };
                let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                continue;
            }

            let mut cmd = Command::new(&req.argv[0]);
            if req.argv.len() > 1 {
                cmd.args(&req.argv[1..]);
            }
            for (k, v) in &req.env {
                cmd.env(k, v);
            }
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            match cmd.spawn() {
                Ok(mut child) => {
                    let mut stdout_data = String::new();
                    let mut stderr_data = String::new();

                    if let Some(mut stdout) = child.stdout.take() {
                        let _ = stdout.read_to_string(&mut stdout_data);
                    }
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut stderr_data);
                    }

                    let status = child.wait().expect("failed to wait on child");
                    let exit_code = status.code().unwrap_or(-1);

                    if !stdout_data.is_empty() {
                        let resp = ExecResponse {
                            msg_type: "stdout".into(),
                            data: Some(stdout_data),
                            code: None,
                        };
                        let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                    }

                    if !stderr_data.is_empty() {
                        let resp = ExecResponse {
                            msg_type: "stderr".into(),
                            data: Some(stderr_data),
                            code: None,
                        };
                        let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                    }

                    let resp = ExecResponse {
                        msg_type: "exit".into(),
                        data: None,
                        code: Some(exit_code),
                    };
                    let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                }
                Err(e) => {
                    let resp = ExecResponse {
                        msg_type: "error".into(),
                        data: Some(format!("failed to spawn: {}", e)),
                        code: None,
                    };
                    let _ = writeln!(writer, "{}", serde_json::to_string(&resp).unwrap());
                }
            }
        }
    }

    extern "C" fn sigchld_handler(_: libc::c_int) {
        // Noop — actual reaping happens in the main loop
    }

    extern "C" fn sigterm_handler(_: libc::c_int) {
        unsafe {
            libc::sync();
            libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF);
        }
    }

    pub fn run() -> ! {
        eprintln!("shuru-guest: starting as PID 1");

        mount_filesystems();
        eprintln!("shuru-guest: filesystems mounted");

        // Set hostname
        let hostname = b"shuru\0";
        unsafe {
            libc::sethostname(hostname.as_ptr() as *const libc::c_char, 5);
        }

        setup_networking();
        eprintln!("shuru-guest: networking ready");

        // Register signal handlers (PID 1 has no default signal dispositions)
        unsafe {
            libc::signal(libc::SIGCHLD, sigchld_handler as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, sigterm_handler as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, sigterm_handler as *const () as libc::sighandler_t);
        }

        let listener_fd = create_vsock_listener(VSOCK_PORT);
        eprintln!("shuru-guest: vsock listening on port {}", VSOCK_PORT);

        loop {
            let client_fd = unsafe {
                libc::accept(listener_fd, std::ptr::null_mut(), std::ptr::null_mut())
            };

            if client_fd < 0 {
                reap_zombies();
                continue;
            }

            eprintln!("shuru-guest: accepted vsock connection");

            std::thread::spawn(move || {
                handle_connection(client_fd);
            });

            reap_zombies();
        }
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    guest::run();

    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("shuru-guest is a Linux-only binary meant to run inside a VM");
        std::process::exit(1);
    }
}
