mod assets;
mod boot;
mod checkpoint;
mod cli;
mod config;
mod measure;
mod stdio;

use std::process;

use anyhow::Result;
use clap::Parser;

use vm::{default_data_dir, VmState};

use cli::{CheckpointCommands, Cli, Commands};
use config::load_config;

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
            vm,
            from,
            console,
            stdio,
            command,
        } => {
            let cfg = load_config(vm.config.as_deref())?;

            // Command resolution: CLI args > config > default /bin/sh
            let command = if !command.is_empty() {
                command
            } else if let Some(cfg_cmd) = cfg.command.clone() {
                cfg_cmd
            } else {
                vec!["/bin/sh".to_string()]
            };

            let plan = boot::plan(&vm, &cfg, from.as_deref())?;
            // What is about to be booted, stated before it is. Taken here,
            // where the source images are still the only thing that exists,
            // and only where something reads it: the stdio wire reports it to
            // whatever is driving the vm. A command run in a vm and thrown
            // away has nobody to tell.
            let launch = if stdio {
                Some(measure::launch(&plan, console, &mut measure::cache(false))?)
            } else {
                None
            };
            let prepared = boot::prepare_vm(plan, !stdio && !console)?;

            let result = if let Some(launch) = &launch {
                stdio::run_stdio(&prepared, launch)
            } else if console {
                run_console(&prepared)
            } else {
                boot::run_command(&prepared, &command).map(|r| r.exit_code)
            };

            // The work rootfs may live outside the instance dir (tmpfs);
            // a no-op when the discard path already unlinked it.
            let _ = std::fs::remove_file(&prepared.work_rootfs);
            let _ = std::fs::remove_dir_all(&prepared.instance_dir);
            boot::trace_boot("instance dir removed");
            process::exit(result?);
        }
        Commands::Measure {
            vm,
            from,
            recompute,
        } => {
            let cfg = load_config(vm.config.as_deref())?;
            let plan = boot::plan(&vm, &cfg, from.as_deref())?;
            let mut cache = measure::cache(recompute);
            println!("{}", measure::document(&plan, false, &mut cache)?);
        }
        Commands::Init { force } => {
            let data_dir = default_data_dir();
            if force {
                let _ = std::fs::remove_file(format!("{}/VERSION", data_dir));
            }
            if assets::assets_ready(&data_dir) {
                eprintln!(
                    "hanzo-vm: OS image already up to date ({})",
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
        Commands::Prune => {
            let data_dir = default_data_dir();
            let instances_dir = format!("{}/instances", data_dir);
            let entries = match std::fs::read_dir(&instances_dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!("hanzo-vm: no orphaned instances found");
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            };

            let mut removed = 0u32;
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let Some(pid) = name.to_str().and_then(|s| s.parse::<i32>().ok()) else {
                    continue;
                };
                // Check if the process is still running
                let alive = unsafe { libc::kill(pid, 0) } == 0;
                if !alive {
                    std::fs::remove_dir_all(entry.path())?;
                    removed += 1;
                }
            }

            // Throwaway work disks of crashed runs may live on tmpfs.
            #[cfg(target_os = "linux")]
            if let Ok(entries) = std::fs::read_dir("/dev/shm/hanzo-vm") {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let Some(pid) = name
                        .to_str()
                        .and_then(|s| s.strip_suffix(".ext4"))
                        .and_then(|s| s.parse::<i32>().ok())
                    else {
                        continue;
                    };
                    if unsafe { libc::kill(pid, 0) } != 0 {
                        let _ = std::fs::remove_file(entry.path());
                        removed += 1;
                    }
                }
            }

            if removed == 0 {
                eprintln!("hanzo-vm: no orphaned instances found");
            } else {
                eprintln!("hanzo-vm: removed {} orphaned instance(s)", removed);
            }
        }
        Commands::Checkpoint { action } => match action {
            CheckpointCommands::Create {
                name,
                vm,
                from,
                command,
            } => {
                let exit_code = checkpoint::create(name, &vm, from.as_deref(), command)?;
                process::exit(exit_code);
            }
            CheckpointCommands::List => checkpoint::list()?,
            CheckpointCommands::Delete { name } => checkpoint::delete(&name)?,
            CheckpointCommands::Push { name: _ } => {
                anyhow::bail!("checkpoint push is not yet implemented")
            }
            CheckpointCommands::Pull { name: _ } => {
                anyhow::bail!("checkpoint pull is not yet implemented")
            }
        },
    }

    Ok(())
}

/// Run the VM in raw serial console mode (for debugging).
fn run_console(prepared: &boot::PreparedVm) -> Result<i32> {
    eprintln!("hanzo-vm: kernel={}", prepared.kernel_path);
    eprintln!("hanzo-vm: rootfs={} (work copy)", prepared.work_rootfs);
    eprintln!(
        "hanzo-vm: booting VM ({}cpus, {}MB RAM, {}MB disk)...",
        prepared.cpus, prepared.memory, prepared.disk_size
    );

    let sandbox = boot::build_sandbox(prepared, true, None, None)?;
    eprintln!("hanzo-vm: VM created and validated successfully");

    let state_rx = sandbox.state_channel();

    eprintln!("hanzo-vm: starting VM...");
    sandbox.start()?;
    eprintln!("hanzo-vm: VM started");

    eprintln!("hanzo-vm: running in console mode (Ctrl+C to stop)");
    let mut exit_code = 0;
    loop {
        match state_rx.recv() {
            Ok(VmState::Stopped) => {
                eprintln!("hanzo-vm: VM stopped");
                break;
            }
            Ok(VmState::Error) => {
                eprintln!("hanzo-vm: VM encountered an error");
                exit_code = 1;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    Ok(exit_code)
}
