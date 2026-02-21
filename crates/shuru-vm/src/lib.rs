#![forbid(unsafe_code)]

mod proto;
mod sandbox;

pub use proto::{ControlMessage, ExecRequest, ExecResponse, ForwardRequest, ForwardResponse, PortMapping};
pub use sandbox::{PortForwardHandle, Sandbox, VmConfigBuilder};

// Re-exports from shuru-darwin for advanced/escape-hatch use
pub use shuru_darwin::VirtualMachine;
pub use shuru_darwin::VmState;
pub use shuru_darwin::VzError;

pub const VSOCK_PORT: u32 = 1024;
pub const VSOCK_PORT_FORWARD: u32 = 1025;

pub fn default_data_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    format!("{}/.local/share/shuru", home)
}
