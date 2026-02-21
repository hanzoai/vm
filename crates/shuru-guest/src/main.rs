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
        #[serde(default)]
        pub tty: bool,
        #[serde(default = "default_rows")]
        pub rows: u16,
        #[serde(default = "default_cols")]
        pub cols: u16,
    }

    fn default_rows() -> u16 {
        24
    }
    fn default_cols() -> u16 {
        80
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

    #[derive(Deserialize)]
    #[serde(tag = "type")]
    pub enum ControlMessage {
        #[serde(rename = "stdin")]
        Stdin { data: String },
        #[serde(rename = "resize")]
        Resize { rows: u16, cols: u16 },
    }

    fn mount_fs(source: &str, target: &str, fstype: &str, data: Option<&str>) {
        use std::ffi::CString;

        let c_source = CString::new(source).unwrap();
        let c_target = CString::new(target).unwrap();
        let c_fstype = CString::new(fstype).unwrap();

        let data_ptr = data.map(|d| CString::new(d).unwrap());
        let ret = unsafe {
            libc::mount(
                c_source.as_ptr(),
                c_target.as_ptr(),
                c_fstype.as_ptr(),
                0,
                data_ptr
                    .as_ref()
                    .map_or(std::ptr::null(), |d| d.as_ptr() as *const libc::c_void),
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
        mount_fs("proc", "/proc", "proc", None);
        mount_fs("sysfs", "/sys", "sysfs", None);
        mount_fs("devtmpfs", "/dev", "devtmpfs", None);
        std::fs::create_dir_all("/dev/pts").ok();
        mount_fs("devpts", "/dev/pts", "devpts", Some("newinstance,ptmxmode=0666"));
        mount_fs("tmpfs", "/tmp", "tmpfs", None);
    }

    fn bring_up_interface(sock: i32, name: &[u8]) {
        unsafe {
            let mut ifr: libc::ifreq = std::mem::zeroed();
            let copy_len = name.len().min(libc::IFNAMSIZ);
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                ifr.ifr_name.as_mut_ptr() as *mut u8,
                copy_len,
            );

            let display_name = String::from_utf8_lossy(&name[..name.len().saturating_sub(1)]);
            if libc::ioctl(sock, libc::SIOCGIFFLAGS as _, &mut ifr) < 0 {
                eprintln!("shuru-guest: failed to get {} flags", display_name);
                return;
            }

            ifr.ifr_ifru.ifru_flags |= libc::IFF_UP as libc::c_short;
            if libc::ioctl(sock, libc::SIOCSIFFLAGS as _, &ifr) < 0 {
                eprintln!("shuru-guest: failed to bring up {}", display_name);
            }
        }
    }

    // --- Native DHCP client ---

    const DHCP_SERVER_PORT: u16 = 67;
    const DHCP_CLIENT_PORT: u16 = 68;
    const DHCP_MAGIC: [u8; 4] = [99, 130, 83, 99];
    const DHCP_DISCOVER: u8 = 1;
    const DHCP_OFFER: u8 = 2;
    const DHCP_REQUEST: u8 = 3;
    const DHCP_ACK: u8 = 5;

    struct DhcpLease {
        ip: [u8; 4],
        subnet: [u8; 4],
        gateway: [u8; 4],
        dns: [u8; 4],
        server_id: [u8; 4],
    }

    fn make_sockaddr_in(ip: [u8; 4], port: u16) -> libc::sockaddr_in {
        libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(ip),
            },
            sin_zero: [0; 8],
        }
    }

    fn get_mac_address(sock: i32) -> Option<[u8; 6]> {
        unsafe {
            let mut ifr: libc::ifreq = std::mem::zeroed();
            std::ptr::copy_nonoverlapping(
                b"eth0\0".as_ptr(),
                ifr.ifr_name.as_mut_ptr() as *mut u8,
                5,
            );
            if libc::ioctl(sock, libc::SIOCGIFHWADDR as _, &mut ifr) < 0 {
                return None;
            }
            let mut mac = [0u8; 6];
            std::ptr::copy_nonoverlapping(
                ifr.ifr_ifru.ifru_hwaddr.sa_data.as_ptr() as *const u8,
                mac.as_mut_ptr(),
                6,
            );
            Some(mac)
        }
    }

    fn build_dhcp_packet(
        msg_type: u8,
        xid: u32,
        mac: &[u8; 6],
        requested_ip: Option<[u8; 4]>,
        server_id: Option<[u8; 4]>,
    ) -> Vec<u8> {
        let mut pkt = vec![0u8; 236];
        pkt[0] = 1; // BOOTREQUEST
        pkt[1] = 1; // Ethernet
        pkt[2] = 6; // MAC length
        pkt[4..8].copy_from_slice(&xid.to_be_bytes());
        pkt[10] = 0x80; // Broadcast flag
        pkt[28..34].copy_from_slice(mac);

        // Magic cookie
        pkt.extend_from_slice(&DHCP_MAGIC);
        // Option 53: DHCP Message Type
        pkt.extend_from_slice(&[53, 1, msg_type]);
        // Option 55: Parameter Request List (subnet, router, DNS)
        pkt.extend_from_slice(&[55, 3, 1, 3, 6]);

        if let Some(ip) = requested_ip {
            pkt.extend_from_slice(&[50, 4]); // Option 50: Requested IP
            pkt.extend_from_slice(&ip);
        }
        if let Some(sid) = server_id {
            pkt.extend_from_slice(&[54, 4]); // Option 54: Server Identifier
            pkt.extend_from_slice(&sid);
        }

        pkt.push(255); // End
        pkt
    }

    fn parse_dhcp_response(pkt: &[u8], expected_xid: u32) -> Option<(u8, DhcpLease)> {
        if pkt.len() < 240 {
            return None;
        }
        if pkt[0] != 2 {
            return None; // Must be BOOTREPLY
        }
        let xid = u32::from_be_bytes([pkt[4], pkt[5], pkt[6], pkt[7]]);
        if xid != expected_xid {
            return None;
        }
        if pkt[236..240] != DHCP_MAGIC {
            return None;
        }

        let ip = [pkt[16], pkt[17], pkt[18], pkt[19]]; // yiaddr
        let mut msg_type = 0u8;
        let mut subnet = [255, 255, 255, 0];
        let mut gateway = [0u8; 4];
        let mut dns = [8, 8, 8, 8]; // fallback
        let mut server_id = [0u8; 4];

        let mut i = 240;
        while i < pkt.len() {
            let opt = pkt[i];
            if opt == 255 {
                break;
            }
            if opt == 0 {
                i += 1;
                continue;
            }
            if i + 1 >= pkt.len() {
                break;
            }
            let len = pkt[i + 1] as usize;
            if i + 2 + len > pkt.len() {
                break;
            }
            match opt {
                53 if len >= 1 => msg_type = pkt[i + 2],
                1 if len >= 4 => subnet.copy_from_slice(&pkt[i + 2..i + 6]),
                3 if len >= 4 => gateway.copy_from_slice(&pkt[i + 2..i + 6]),
                6 if len >= 4 => dns.copy_from_slice(&pkt[i + 2..i + 6]),
                54 if len >= 4 => server_id.copy_from_slice(&pkt[i + 2..i + 6]),
                _ => {}
            }
            i += 2 + len;
        }

        Some((
            msg_type,
            DhcpLease {
                ip,
                subnet,
                gateway,
                dns,
                server_id,
            },
        ))
    }

    fn dhcp_request(mac: &[u8; 6]) -> Option<DhcpLease> {
        unsafe {
            let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_UDP);
            if sock < 0 {
                return None;
            }

            let one: libc::c_int = 1;
            libc::setsockopt(
                sock,
                libc::SOL_SOCKET,
                libc::SO_BROADCAST,
                &one as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );

            // Bind to eth0 so DHCP goes through the right interface
            libc::setsockopt(
                sock,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                b"eth0\0".as_ptr() as *const libc::c_void,
                5,
            );

            let tv = libc::timeval {
                tv_sec: 5,
                tv_usec: 0,
            };
            libc::setsockopt(
                sock,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &tv as *const _ as *const libc::c_void,
                std::mem::size_of::<libc::timeval>() as libc::socklen_t,
            );

            let bind_addr = make_sockaddr_in([0, 0, 0, 0], DHCP_CLIENT_PORT);
            if libc::bind(
                sock,
                &bind_addr as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            ) < 0
            {
                eprintln!(
                    "shuru-guest: DHCP bind failed: {}",
                    std::io::Error::last_os_error()
                );
                libc::close(sock);
                return None;
            }

            let broadcast = make_sockaddr_in([255, 255, 255, 255], DHCP_SERVER_PORT);
            let xid = libc::getpid() as u32;

            // DHCPDISCOVER
            let discover = build_dhcp_packet(DHCP_DISCOVER, xid, mac, None, None);
            if libc::sendto(
                sock,
                discover.as_ptr() as *const libc::c_void,
                discover.len(),
                0,
                &broadcast as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            ) < 0
            {
                libc::close(sock);
                return None;
            }

            // Receive DHCPOFFER
            let mut buf = [0u8; 1500];
            let n = libc::recv(sock, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            if n <= 0 {
                eprintln!("shuru-guest: DHCP no offer received");
                libc::close(sock);
                return None;
            }

            let (msg_type, offer) = match parse_dhcp_response(&buf[..n as usize], xid) {
                Some(v) => v,
                None => {
                    libc::close(sock);
                    return None;
                }
            };
            if msg_type != DHCP_OFFER {
                libc::close(sock);
                return None;
            }

            // DHCPREQUEST
            let request =
                build_dhcp_packet(DHCP_REQUEST, xid, mac, Some(offer.ip), Some(offer.server_id));
            libc::sendto(
                sock,
                request.as_ptr() as *const libc::c_void,
                request.len(),
                0,
                &broadcast as *const _ as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );

            // Receive DHCPACK
            let n = libc::recv(sock, buf.as_mut_ptr() as *mut libc::c_void, buf.len(), 0);
            libc::close(sock);
            if n <= 0 {
                return None;
            }

            let (msg_type, ack) = parse_dhcp_response(&buf[..n as usize], xid)?;
            if msg_type == DHCP_ACK {
                Some(ack)
            } else {
                None
            }
        }
    }

    // --- Interface configuration via ioctl ---

    fn set_interface_addr(sock: i32, ip: [u8; 4], mask: [u8; 4]) {
        unsafe {
            let mut ifr: libc::ifreq = std::mem::zeroed();
            std::ptr::copy_nonoverlapping(
                b"eth0\0".as_ptr(),
                ifr.ifr_name.as_mut_ptr() as *mut u8,
                5,
            );

            // Set IP address
            let addr = make_sockaddr_in(ip, 0);
            std::ptr::copy_nonoverlapping(
                &addr as *const _ as *const u8,
                &mut ifr.ifr_ifru as *mut _ as *mut u8,
                std::mem::size_of::<libc::sockaddr_in>(),
            );
            if libc::ioctl(sock, libc::SIOCSIFADDR as _, &ifr) < 0 {
                eprintln!(
                    "shuru-guest: failed to set IP: {}",
                    std::io::Error::last_os_error()
                );
            }

            // Set subnet mask
            let mask_addr = make_sockaddr_in(mask, 0);
            std::ptr::copy_nonoverlapping(
                &mask_addr as *const _ as *const u8,
                &mut ifr.ifr_ifru as *mut _ as *mut u8,
                std::mem::size_of::<libc::sockaddr_in>(),
            );
            if libc::ioctl(sock, libc::SIOCSIFNETMASK as _, &ifr) < 0 {
                eprintln!(
                    "shuru-guest: failed to set netmask: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    // Linux rtentry for SIOCADDRT (not in libc crate)
    #[repr(C)]
    struct RtEntry {
        rt_pad1: libc::c_ulong,
        rt_dst: libc::sockaddr,
        rt_gateway: libc::sockaddr,
        rt_genmask: libc::sockaddr,
        rt_flags: libc::c_ushort,
        rt_pad2: libc::c_short,
        rt_pad3: libc::c_ulong,
        rt_pad4: *mut libc::c_void,
        rt_metric: libc::c_short,
        rt_dev: *mut libc::c_char,
        rt_mtu: libc::c_ulong,
        rt_window: libc::c_ulong,
        rt_irtt: libc::c_ushort,
    }

    fn add_default_route(sock: i32, gateway: [u8; 4]) {
        unsafe {
            let mut rt: RtEntry = std::mem::zeroed();

            let dst = make_sockaddr_in([0, 0, 0, 0], 0);
            std::ptr::copy_nonoverlapping(
                &dst as *const _ as *const u8,
                &mut rt.rt_dst as *mut _ as *mut u8,
                std::mem::size_of::<libc::sockaddr>(),
            );

            let gw = make_sockaddr_in(gateway, 0);
            std::ptr::copy_nonoverlapping(
                &gw as *const _ as *const u8,
                &mut rt.rt_gateway as *mut _ as *mut u8,
                std::mem::size_of::<libc::sockaddr>(),
            );

            let mask = make_sockaddr_in([0, 0, 0, 0], 0);
            std::ptr::copy_nonoverlapping(
                &mask as *const _ as *const u8,
                &mut rt.rt_genmask as *mut _ as *mut u8,
                std::mem::size_of::<libc::sockaddr>(),
            );

            rt.rt_flags = libc::RTF_UP | libc::RTF_GATEWAY;

            if libc::ioctl(sock, libc::SIOCADDRT as _, &rt) < 0 {
                eprintln!(
                    "shuru-guest: failed to add default route: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    fn fmt_ip(ip: [u8; 4]) -> String {
        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
    }

    // --- Main networking setup ---

    fn setup_networking() {
        unsafe {
            let sock = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if sock < 0 {
                eprintln!("shuru-guest: failed to create socket for networking setup");
                return;
            }

            bring_up_interface(sock, b"lo\0");

            // Check if eth0 exists (network device present)
            let has_eth0 = {
                let mut ifr: libc::ifreq = std::mem::zeroed();
                std::ptr::copy_nonoverlapping(
                    b"eth0\0".as_ptr(),
                    ifr.ifr_name.as_mut_ptr() as *mut u8,
                    5,
                );
                libc::ioctl(sock, libc::SIOCGIFFLAGS as _, &mut ifr) == 0
            };

            if !has_eth0 {
                libc::close(sock);
                eprintln!("shuru-guest: no network device (sandbox mode)");
                return;
            }

            bring_up_interface(sock, b"eth0\0");

            // Check if eth0 already has an IP (configured by initramfs DHCP)
            let already_configured = {
                let mut ifr: libc::ifreq = std::mem::zeroed();
                std::ptr::copy_nonoverlapping(
                    b"eth0\0".as_ptr(),
                    ifr.ifr_name.as_mut_ptr() as *mut u8,
                    5,
                );
                libc::ioctl(sock, libc::SIOCGIFADDR as _, &mut ifr) == 0
            };

            if already_configured {
                eprintln!("shuru-guest: network already configured (by initramfs)");
                libc::close(sock);
                return;
            }

            // Fallback: DHCP in userspace if initramfs didn't configure networking
            let mac = match get_mac_address(sock) {
                Some(m) => m,
                None => {
                    eprintln!("shuru-guest: failed to get MAC address");
                    libc::close(sock);
                    return;
                }
            };

            match dhcp_request(&mac) {
                Some(lease) => {
                    set_interface_addr(sock, lease.ip, lease.subnet);
                    add_default_route(sock, lease.gateway);

                    let dns_conf = format!("nameserver {}\n", fmt_ip(lease.dns));
                    let _ = std::fs::write("/etc/resolv.conf", dns_conf);

                    eprintln!(
                        "shuru-guest: network configured: ip={} gw={}",
                        fmt_ip(lease.ip),
                        fmt_ip(lease.gateway)
                    );
                }
                None => {
                    eprintln!("shuru-guest: DHCP failed, no network");
                }
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

    fn send_response(fd: i32, resp: &ExecResponse) {
        let json = serde_json::to_string(resp).unwrap();
        let msg = format!("{}\n", json);
        unsafe {
            libc::write(fd, msg.as_ptr() as *const libc::c_void, msg.len());
        }
    }

    fn send_error(fd: i32, msg: &str) {
        send_response(
            fd,
            &ExecResponse {
                msg_type: "error".into(),
                data: Some(msg.into()),
                code: None,
            },
        );
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

            if req.tty {
                // TTY mode: hand off the raw fd, the line-based protocol is over
                let raw_fd = std::os::unix::io::AsRawFd::as_raw_fd(&writer);
                // Prevent TcpStream from closing the fd on drop
                std::mem::forget(writer);
                handle_tty_exec(raw_fd, &req);
                return;
            }

            // Non-TTY mode: piped exec (original behavior)
            handle_piped_exec(&req, &mut writer);
        }
    }

    fn handle_piped_exec(req: &ExecRequest, writer: &mut impl Write) {
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

    fn handle_tty_exec(vsock_fd: i32, req: &ExecRequest) {
        use std::ffi::CString;

        unsafe {
            // Set up initial winsize
            let ws = libc::winsize {
                ws_row: req.rows,
                ws_col: req.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };

            // Allocate PTY pair
            let mut master: libc::c_int = 0;
            let mut slave: libc::c_int = 0;
            if libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &ws as *const libc::winsize as *mut libc::winsize,
            ) < 0
            {
                send_error(vsock_fd, "openpty failed");
                libc::close(vsock_fd);
                return;
            }

            let pid = libc::fork();
            if pid < 0 {
                send_error(vsock_fd, "fork failed");
                libc::close(master);
                libc::close(slave);
                libc::close(vsock_fd);
                return;
            }

            if pid == 0 {
                // === CHILD ===
                libc::close(master);
                libc::close(vsock_fd);
                libc::setsid();
                libc::ioctl(slave, libc::TIOCSCTTY, 0);
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                if slave > 2 {
                    libc::close(slave);
                }

                // Close any other inherited fds
                for fd in 3..1024 {
                    libc::close(fd);
                }

                // Set environment
                for (k, v) in &req.env {
                    if let Ok(var) = CString::new(format!("{}={}", k, v)) {
                        libc::putenv(var.into_raw());
                    }
                }
                if !req.env.contains_key("TERM") {
                    let term = CString::new("TERM=xterm-256color").unwrap();
                    libc::putenv(term.into_raw());
                }
                if !req.env.contains_key("PATH") {
                    let path = CString::new(
                        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                    )
                    .unwrap();
                    libc::putenv(path.into_raw());
                }

                // Build argv and exec
                let c_args: Vec<CString> = req
                    .argv
                    .iter()
                    .map(|s| CString::new(s.as_str()).unwrap_or_else(|_| CString::new("").unwrap()))
                    .collect();
                let c_argv: Vec<*const libc::c_char> = c_args
                    .iter()
                    .map(|s| s.as_ptr())
                    .chain(std::iter::once(std::ptr::null()))
                    .collect();

                libc::execvp(c_argv[0], c_argv.as_ptr());

                // If execvp returns, it failed
                libc::_exit(127);
            }

            // === PARENT ===
            libc::close(slave);
            pty_poll_loop(vsock_fd, master, pid);
            libc::close(master);
            libc::close(vsock_fd);
        }
    }

    fn pty_poll_loop(vsock_fd: i32, master_fd: i32, child_pid: libc::pid_t) {
        let mut vsock_buf: Vec<u8> = Vec::new();
        let mut read_buf = [0u8; 4096];

        loop {
            let mut fds = [
                libc::pollfd {
                    fd: vsock_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: master_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let ret = unsafe { libc::poll(fds.as_mut_ptr(), 2, 200) };
            if ret < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                break;
            }

            // Check vsock for control messages (stdin, resize)
            if fds[0].revents & libc::POLLIN != 0 {
                let n = unsafe {
                    libc::read(
                        vsock_fd,
                        read_buf.as_mut_ptr() as *mut libc::c_void,
                        read_buf.len(),
                    )
                };
                if n <= 0 {
                    // Host disconnected — signal child and exit
                    unsafe {
                        libc::kill(child_pid, libc::SIGHUP);
                    }
                    break;
                }
                vsock_buf.extend_from_slice(&read_buf[..n as usize]);

                // Process complete JSON lines
                while let Some(pos) = vsock_buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = vsock_buf.drain(..=pos).collect();
                    let line_str = String::from_utf8_lossy(&line);
                    let line_str = line_str.trim();
                    if line_str.is_empty() {
                        continue;
                    }

                    if let Ok(msg) = serde_json::from_str::<ControlMessage>(line_str) {
                        match msg {
                            ControlMessage::Stdin { data } => {
                                let bytes = data.as_bytes();
                                unsafe {
                                    libc::write(
                                        master_fd,
                                        bytes.as_ptr() as *const libc::c_void,
                                        bytes.len(),
                                    );
                                }
                            }
                            ControlMessage::Resize { rows, cols } => unsafe {
                                let ws = libc::winsize {
                                    ws_row: rows,
                                    ws_col: cols,
                                    ws_xpixel: 0,
                                    ws_ypixel: 0,
                                };
                                libc::ioctl(master_fd, libc::TIOCSWINSZ, &ws);
                            },
                        }
                    }
                }
            }

            if fds[0].revents & (libc::POLLHUP | libc::POLLERR) != 0 {
                unsafe {
                    libc::kill(child_pid, libc::SIGHUP);
                }
                break;
            }

            // Check PTY master for output
            if fds[1].revents & libc::POLLIN != 0 {
                let n = unsafe {
                    libc::read(
                        master_fd,
                        read_buf.as_mut_ptr() as *mut libc::c_void,
                        read_buf.len(),
                    )
                };
                if n > 0 {
                    let data = String::from_utf8_lossy(&read_buf[..n as usize]);
                    send_response(
                        vsock_fd,
                        &ExecResponse {
                            msg_type: "stdout".into(),
                            data: Some(data.into_owned()),
                            code: None,
                        },
                    );
                }
            }

            if fds[1].revents & libc::POLLHUP != 0 {
                // Child closed PTY — drain remaining output
                loop {
                    let n = unsafe {
                        libc::read(
                            master_fd,
                            read_buf.as_mut_ptr() as *mut libc::c_void,
                            read_buf.len(),
                        )
                    };
                    if n <= 0 {
                        break;
                    }
                    let data = String::from_utf8_lossy(&read_buf[..n as usize]);
                    send_response(
                        vsock_fd,
                        &ExecResponse {
                            msg_type: "stdout".into(),
                            data: Some(data.into_owned()),
                            code: None,
                        },
                    );
                }
                break;
            }
        }

        // Wait for child and send exit code
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(child_pid, &mut status, 0);
        }
        let exit_code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            128 + libc::WTERMSIG(status)
        } else {
            1
        };

        send_response(
            vsock_fd,
            &ExecResponse {
                msg_type: "exit".into(),
                data: None,
                code: Some(exit_code),
            },
        );
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
