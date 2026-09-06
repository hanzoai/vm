#![deny(unsafe_code)]

#[cfg(target_os = "linux")]
mod clone;
mod sandbox;

#[cfg(target_os = "linux")]
pub use clone::{clone_file, reflink_file};

pub use sandbox::{command_line, MountConfig, PortForwardHandle, Sandbox, VmConfigBuilder};
pub use vm_proto::{
    frame, ExecRequest, ForwardRequest, ForwardResponse, MountRequest, MountResponse, PortMapping,
    ReadFileRequest, WriteFileRequest, WriteFileResponse, VSOCK_PORT, VSOCK_PORT_FORWARD,
};

// The platform backend: Virtualization.framework on macOS, KVM on arm64
// Linux, cloud-hypervisor on x86_64 Linux. All expose the same vocabulary.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub(crate) use vm_ch as backend;
#[cfg(target_os = "macos")]
pub(crate) use vm_darwin as backend;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) use vm_linux as backend;

// Re-exports from the backend for advanced/escape-hatch use
pub use backend::{VirtualMachine, VmState, VzError};

/// Reject checkpoint names that could escape the checkpoints directory.
pub fn validate_checkpoint_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("checkpoint name cannot be empty".into());
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') || name.contains("..") {
        return Err(format!("invalid checkpoint name: '{}'", name));
    }
    Ok(())
}

/// `HANZO_VM_HOME`, else `~/.hanzo/vm`.
pub fn default_data_dir() -> String {
    if let Ok(dir) = std::env::var("HANZO_VM_HOME") {
        return dir;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.hanzo/vm", home)
}
