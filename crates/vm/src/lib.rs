#![forbid(unsafe_code)]

mod sandbox;

pub use sandbox::{MountConfig, PortForwardHandle, Sandbox, VmConfigBuilder};
pub use vm_proto::{
    frame, ExecRequest, ForwardRequest, ForwardResponse, MountRequest, MountResponse, PortMapping,
    ReadFileRequest, WriteFileRequest, WriteFileResponse, VSOCK_PORT, VSOCK_PORT_FORWARD,
};

// Re-exports from platform-specific backend for advanced/escape-hatch use
#[cfg(target_os = "macos")]
pub use vm_darwin::VirtualMachine;
#[cfg(target_os = "macos")]
pub use vm_darwin::VmState;
#[cfg(target_os = "macos")]
pub use vm_darwin::VzError;

#[cfg(target_os = "linux")]
pub use vm_linux::VirtualMachine;
#[cfg(target_os = "linux")]
pub use vm_linux::VmState;
#[cfg(target_os = "linux")]
pub use vm_linux::VzError;

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
