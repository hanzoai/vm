mod assets;

use std::collections::HashMap;
use std::io::IsTerminal;
use std::process;

use anyhow::{bail, Result};
use clap::Parser;
use serde::Deserialize;

use shuru_vm::{default_data_dir, Sandbox, VmState};

#[derive(Default, Deserialize)]
struct ShuruConfig {
    cpus: Option<usize>,
    memory: Option<u64>,
    allow_net: Option<bool>,
    command: Option<Vec<String>>,
}

fn load_config(config_flag: Option<&str>) -> Result<ShuruConfig> {
    let path = match config_flag {
        Some(p) => std::path::PathBuf::from(p),
        None => std::path::PathBuf::from("shuru.json"),
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let cfg: ShuruConfig = serde_json::from_str(&contents)
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if config_flag.is_some() {
                bail!("Config file not found: {}", path.display());
            }
            Ok(ShuruConfig::default())
        }
        Err(e) => bail!("Failed to read {}: {}", path.display(), e),
    }
}

#[derive(Parser)]
#[command(name = "shuru", about = "microVM sandbox for AI agents", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Boot a VM and run a command inside it
    Run {
        /// Number of CPU cores
        #[arg(long)]
        cpus: Option<usize>,

        /// Memory in MB
        #[arg(long)]
        memory: Option<u64>,

        /// Path to kernel
        #[arg(long, env = "SHURU_KERNEL")]
        kernel: Option<String>,

        /// Path to rootfs image
        #[arg(long, env = "SHURU_ROOTFS")]
        rootfs: Option<String>,

        /// Path to initramfs (for loading VirtIO modules)
        #[arg(long, env = "SHURU_INITRD")]
        initrd: Option<String>,

        /// Allow network access (NAT)
        #[arg(long)]
        allow_net: bool,

        /// Path to config file (default: ./shuru.json)
        #[arg(long)]
        config: Option<String>,

        /// Attach to raw serial console instead of running a command
        #[arg(long)]
        console: bool,

        /// Command and arguments to run inside the VM
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Download or update OS image assets
    Init {
        /// Force re-download even if assets exist
        #[arg(long)]
        force: bool,
    },

    /// Upgrade shuru to the latest release (CLI + OS image)
    Upgrade,
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
            allow_net,
            config,
            console,
            command,
        } => {
            let cfg = load_config(config.as_deref())?;

            // Resolution: CLI flag > config file > hardcoded default
            let cpus = cpus.or(cfg.cpus).unwrap_or(2);
            let memory = memory.or(cfg.memory).unwrap_or(2048);
            let allow_net = allow_net || cfg.allow_net.unwrap_or(false);

            // Command resolution: CLI args > config > default /bin/sh
            let command = if !command.is_empty() {
                command
            } else if let Some(cfg_cmd) = cfg.command {
                cfg_cmd
            } else {
                vec!["/bin/sh".to_string()]
            };

            let data_dir = default_data_dir();

            // Auto-download assets when using default paths
            if kernel.is_none()
                && rootfs.is_none()
                && initrd.is_none()
                && !assets::assets_ready(&data_dir)
            {
                assets::download_os_image(&data_dir)?;
            }

            let kernel_path = kernel.unwrap_or_else(|| format!("{}/Image", data_dir));
            let rootfs_path = rootfs.unwrap_or_else(|| format!("{}/rootfs.ext4", data_dir));
            let initrd_path = initrd.unwrap_or_else(|| format!("{}/initramfs.cpio.gz", data_dir));

            if !std::path::Path::new(&kernel_path).exists() {
                bail!(
                    "Kernel not found at {}. Run `shuru init` to download.",
                    kernel_path
                );
            }
            if !std::path::Path::new(&rootfs_path).exists() {
                bail!(
                    "Rootfs not found at {}. Run `shuru init` to download.",
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
                .memory_mb(memory)
                .allow_net(allow_net);

            if let Some(initrd) = initrd_opt {
                eprintln!("shuru: using initramfs: {}", initrd);
                builder = builder.initrd(initrd);
            }

            // In console mode, keep serial stdin; otherwise disconnect it
            if !console {
                builder = builder.console(false);
            }

            let sandbox = builder.build()?;
            eprintln!("shuru: VM created and validated successfully");

            let state_rx = sandbox.state_channel();

            eprintln!("shuru: starting VM...");
            sandbox.start()?;
            eprintln!("shuru: VM started");

            if console {
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
        Commands::Init { force } => {
            let data_dir = default_data_dir();
            if force {
                let _ = std::fs::remove_file(format!("{}/VERSION", data_dir));
            }
            if assets::assets_ready(&data_dir) {
                eprintln!(
                    "shuru: OS image already up to date ({})",
                    assets::CURRENT_VERSION
                );
            } else {
                assets::download_os_image(&data_dir)?;
            }
        }
        Commands::Upgrade => {
            let data_dir = default_data_dir();
            assets::upgrade(&data_dir)?;
        }
    }

    Ok(())
}
