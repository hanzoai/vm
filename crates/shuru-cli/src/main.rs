use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::FromRawFd;
use std::process;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};

use shuru_vz::*;

const VSOCK_PORT: u32 = 1024;

#[derive(Parser)]
#[command(name = "shuru", about = "microVM sandbox for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Boot a VM and optionally run a command inside it
    Run {
        /// Number of CPU cores
        #[arg(long, default_value = "2")]
        cpus: usize,

        /// Memory in MB
        #[arg(long, default_value = "2048")]
        memory: u64,

        /// Path to kernel
        #[arg(long, env = "SHURU_KERNEL")]
        kernel: Option<String>,

        /// Path to rootfs image
        #[arg(long, env = "SHURU_ROOTFS")]
        rootfs: Option<String>,

        /// Path to initramfs (for loading VirtIO modules)
        #[arg(long, env = "SHURU_INITRD")]
        initrd: Option<String>,

        /// Command and arguments to run inside the VM
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

#[derive(Serialize)]
struct ExecRequest {
    argv: Vec<String>,
    env: std::collections::HashMap<String, String>,
}

#[derive(Deserialize)]
struct ExecResponse {
    #[serde(rename = "type")]
    msg_type: String,
    data: Option<String>,
    code: Option<i32>,
}

fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/shuru", home)
}

fn create_vm(
    kernel_path: &str,
    rootfs_path: &str,
    initrd_path: Option<&str>,
    cpus: usize,
    memory_mb: u64,
) -> Result<VirtualMachine> {
    if !VirtualMachine::supported() {
        bail!("Virtualization is not supported on this machine");
    }

    eprintln!("shuru: configuring boot loader...");
    let boot_loader = LinuxBootLoader::new_with_kernel(kernel_path);
    if let Some(initrd) = initrd_path {
        eprintln!("shuru: using initramfs: {}", initrd);
        boot_loader.set_initrd(initrd);
    }
    boot_loader.set_command_line("console=hvc0 root=/dev/vda rw");

    let memory_bytes = memory_mb * 1024 * 1024;
    let config = VirtualMachineConfiguration::new(boot_loader, cpus, memory_bytes);

    // Serial console -> stdout for guest output, stdin for guest input
    eprintln!("shuru: configuring serial console...");
    let serial_attachment =
        FileHandleSerialPortAttachment::new(&std::io::stdin(), &std::io::stdout());
    let serial = VirtioConsoleDeviceSerialPortConfiguration::new_with_attachment(serial_attachment);
    config.set_serial_ports(vec![serial]);

    // Rootfs disk
    eprintln!("shuru: configuring storage...");
    let disk_attachment = DiskImageStorageDeviceAttachment::new(rootfs_path, false);
    let block_device = VirtioBlockDeviceConfiguration::new(disk_attachment);
    config.set_storage_devices(vec![block_device]);

    // NAT network
    eprintln!("shuru: configuring network...");
    let net_attachment = NATNetworkDeviceAttachment::new();
    let net_device = VirtioNetworkDeviceConfiguration::new_with_attachment(net_attachment);
    net_device.set_mac_address(MACAddress::new_with_random_locally_administered_address());
    config.set_network_devices(vec![net_device]);

    // vsock
    eprintln!("shuru: configuring vsock...");
    let socket_device = VirtioSocketDeviceConfiguration::new();
    config.set_socket_devices(vec![socket_device]);

    // Entropy
    config.set_entropy_devices(vec![VirtioEntropyDeviceConfiguration::new()]);

    // Memory balloon
    config.set_memory_balloon_devices(vec![
        VirtioTraditionalMemoryBalloonDeviceConfiguration::new(),
    ]);

    eprintln!("shuru: validating configuration...");
    config
        .validate()
        .map_err(|e| anyhow::anyhow!("VM configuration invalid: {}", e))?;

    eprintln!("shuru: creating virtual machine...");
    Ok(VirtualMachine::new(&config))
}

fn exec_command(fd: i32, argv: Vec<String>) -> Result<i32> {
    // SAFETY: fd is a valid socket from vsock connect
    let stream = unsafe { std::net::TcpStream::from_raw_fd(fd) };
    let mut writer = stream.try_clone()?;
    let reader = BufReader::new(stream);

    let req = ExecRequest {
        argv,
        env: std::collections::HashMap::new(),
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
                    print!("{}", data);
                }
            }
            "stderr" => {
                if let Some(data) = &resp.data {
                    eprint!("{}", data);
                }
            }
            "exit" => {
                exit_code = resp.code.unwrap_or(0);
                break;
            }
            "error" => {
                if let Some(data) = &resp.data {
                    eprintln!("shuru: guest error: {}", data);
                }
                exit_code = 1;
                break;
            }
            _ => {}
        }
    }

    Ok(exit_code)
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            cpus,
            memory,
            kernel,
            rootfs,
            initrd,
            command,
        } => {
            let data_dir = default_data_dir();
            let kernel_path = kernel.unwrap_or_else(|| format!("{}/Image", data_dir));
            let rootfs_path = rootfs.unwrap_or_else(|| format!("{}/rootfs.ext4", data_dir));
            let initrd_path = initrd.unwrap_or_else(|| format!("{}/initramfs.cpio.gz", data_dir));

            if !std::path::Path::new(&kernel_path).exists() {
                bail!(
                    "Kernel not found at {}. Run scripts/prepare-rootfs.sh first.",
                    kernel_path
                );
            }
            if !std::path::Path::new(&rootfs_path).exists() {
                bail!(
                    "Rootfs not found at {}. Run scripts/prepare-rootfs.sh first.",
                    rootfs_path
                );
            }

            // Initrd is optional — if not present, boot without it
            let initrd_opt = if std::path::Path::new(&initrd_path).exists() {
                Some(initrd_path.as_str())
            } else {
                eprintln!("shuru: warning: initramfs not found at {}, booting without it", initrd_path);
                None
            };

            eprintln!("shuru: kernel={}", kernel_path);
            eprintln!("shuru: rootfs={}", rootfs_path);
            eprintln!("shuru: booting VM ({}cpus, {}MB RAM)...", cpus, memory);

            let vm = create_vm(&kernel_path, &rootfs_path, initrd_opt, cpus, memory)?;
            eprintln!("shuru: VM created and validated successfully");
            let state_rx = vm.get_state_channel();

            eprintln!("shuru: starting VM...");
            vm.start()
                .map_err(|e| anyhow::anyhow!("Failed to start VM: {}", e))?;
            eprintln!("shuru: VM started");

            if command.is_empty() {
                // Console mode — serial output goes to stdout
                eprintln!("shuru: running in console mode (Ctrl+C to stop)");
                loop {
                    match state_rx.recv() {
                        Ok(VirtualMachineState::Stopped) => {
                            eprintln!("shuru: VM stopped");
                            break;
                        }
                        Ok(VirtualMachineState::Error) => {
                            eprintln!("shuru: VM encountered an error");
                            process::exit(1);
                        }
                        Ok(_) => continue,
                        Err(_) => break,
                    }
                }
            } else {
                // Exec mode — connect via vsock and run command
                eprintln!("shuru: waiting for guest to be ready...");
                std::thread::sleep(Duration::from_secs(3));

                let mut fd = None;
                for attempt in 1..=10 {
                    match vm.connect_to_vsock_port(VSOCK_PORT) {
                        Ok(f) => {
                            fd = Some(f);
                            break;
                        }
                        Err(e) => {
                            if attempt == 10 {
                                bail!("Failed to connect to guest after 10 attempts: {}", e);
                            }
                            tracing::debug!("vsock connect attempt {} failed: {}", attempt, e);
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                }

                let fd = fd.unwrap();
                eprintln!("shuru: connected to guest, executing command...");

                let exit_code = exec_command(fd, command)?;

                let _ = vm.stop();
                process::exit(exit_code);
            }
        }
    }

    Ok(())
}
