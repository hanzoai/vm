//! What this launch is, as a number.
//!
//! Everything the hypervisor is handed — the kernel, the command line, the
//! initramfs when there is one, the root image, the shape of the machine — is
//! measured into one register before the VM starts. The vm can only speak for
//! the launch: what the started system then runs is the caller's to extend
//! (`hanzo up` adds the workload it deploys), which is why this module
//! produces a [`Log`] and not a finished document.
//!
//! The root image measured is the SOURCE — the base image or the checkpoint —
//! never the per-instance working copy. The copy is a clone of the source
//! extended with zeros to `--disk-size` and is written to during the run; the
//! source is the thing two machines can compare.

use anyhow::Result;
use vm_measure::{Cache, Log, Measurement};

use crate::boot::Plan;

/// Where remembered file digests live: beside the images they describe.
pub(crate) fn cache(recompute: bool) -> Cache {
    if recompute {
        return Cache::none();
    }
    Cache::open(format!("{}/digests.json", vm::default_data_dir()))
}

/// The launch log for a planned VM. `console` completes the command line:
/// naming a console changes what the kernel is told, so it changes what the
/// guest is.
pub(crate) fn launch(plan: &Plan, console: bool, cache: &mut Cache) -> Result<Log> {
    let mut log = Log::default();
    log.file("kernel", plan.kernel_path.as_ref(), cache)?;
    log.text("cmdline", &vm::command_line(plan.verbose, console));
    if let Some(initrd) = &plan.initrd_path {
        log.file("initrd", initrd.as_ref(), cache)?;
    }
    log.file("root", plan.source_rootfs.as_ref(), cache)?;
    log.text("shape", &shape(plan));
    cache.save();
    Ok(log)
}

/// The machine the guest wakes up on. Measured because a platform that signs
/// for a launch signs for its vCPU count and memory too — a verifier that
/// checked only the software would accept the same kernel on a machine it
/// never agreed to.
fn shape(plan: &Plan) -> String {
    format!(
        "cpus={} memory={}MB disk={}MB",
        plan.cpus, plan.memory, plan.disk_size
    )
}

/// The whole document for a launch nothing has been deployed into yet.
///
/// `hardware` is `none` and stays `none`: a platform is asked from inside the
/// guest, and this states what a boot WOULD be — there is no guest to ask.
pub(crate) fn document(plan: &Plan, console: bool, cache: &mut Cache) -> Result<String> {
    let m = Measurement {
        launch: launch(plan, console, cache)?,
        ..Measurement::default()
    };
    Ok(m.to_json_pretty())
}

/// Remember a newly written image's digest while the slow path is already
/// running, so the boot that follows measures from the cache in microseconds
/// instead of reading gigabytes.
pub(crate) fn warm(path: &str) {
    let mut cache = cache(false);
    if cache.digest(path.as_ref()).is_ok() {
        cache.save();
    }
}
