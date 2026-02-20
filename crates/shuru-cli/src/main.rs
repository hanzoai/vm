use std::collections::HashMap;
use std::io::IsTerminal;
use std::process;

use anyhow::{bail, Result};
use clap::Parser;

use shuru_vm::{default_data_dir, Sandbox, VmState};

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

            let initrd_opt = if std::path::Path::new(&initrd_path).exists() {
                Some(initrd_path.as_str())
            } else {
                eprintln!(
                    "shuru: warning: initramfs not found at {}, booting without it",
                    initrd_path
                );
                None
            };

            eprintln!("shuru: kernel={}", kernel_path);
            eprintln!("shuru: rootfs={}", rootfs_path);
            eprintln!("shuru: booting VM ({}cpus, {}MB RAM)...", cpus, memory);

            let mut builder = Sandbox::builder()
                .kernel(&kernel_path)
                .rootfs(&rootfs_path)
                .cpus(cpus)
                .memory_mb(memory);

            if let Some(initrd) = initrd_opt {
                eprintln!("shuru: using initramfs: {}", initrd);
                builder = builder.initrd(initrd);
            }

            // Disable serial console stdin in exec/shell mode
            if !command.is_empty() {
                builder = builder.console(false);
            }

            let sandbox = builder.build()?;
            eprintln!("shuru: VM created and validated successfully");

            let state_rx = sandbox.state_channel();

            eprintln!("shuru: starting VM...");
            sandbox.start()?;
            eprintln!("shuru: VM started");

            if command.is_empty() {
                eprintln!("shuru: running in console mode (Ctrl+C to stop)");
                loop {
                    match state_rx.recv() {
                        Ok(VmState::Stopped) => {
                            eprintln!("shuru: VM stopped");
                            break;
                        }
                        Ok(VmState::Error) => {
                            eprintln!("shuru: VM encountered an error");
                            process::exit(1);
                        }
                        Ok(_) => continue,
                        Err(_) => break,
                    }
                }
            } else {
                eprintln!("shuru: waiting for guest to be ready...");

                let exit_code = if std::io::stdin().is_terminal() {
                    sandbox.shell(&command, &HashMap::new())?
                } else {
                    sandbox.exec(&command, &mut std::io::stdout(), &mut std::io::stderr())?
                };

                let _ = sandbox.stop();
                process::exit(exit_code);
            }
        }
    }

    Ok(())
}
