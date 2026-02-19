#![forbid(unsafe_code)]

pub use shuru_darwin::DiskImageAttachment as DiskImageStorageDeviceAttachment;
pub use shuru_darwin::FileHandleSerialAttachment as FileHandleSerialPortAttachment;
pub use shuru_darwin::LinuxBootLoader;
pub use shuru_darwin::MACAddress;
pub use shuru_darwin::NATNetworkAttachment as NATNetworkDeviceAttachment;
pub use shuru_darwin::VirtioBlockDevice as VirtioBlockDeviceConfiguration;
pub use shuru_darwin::VirtioConsoleSerialPort as VirtioConsoleDeviceSerialPortConfiguration;
pub use shuru_darwin::VirtioEntropyDevice as VirtioEntropyDeviceConfiguration;
pub use shuru_darwin::VirtioMemoryBalloonDevice as VirtioTraditionalMemoryBalloonDeviceConfiguration;
pub use shuru_darwin::VirtioNetworkDevice as VirtioNetworkDeviceConfiguration;
pub use shuru_darwin::VirtioSocketDevice as VirtioSocketDeviceConfiguration;
pub use shuru_darwin::VirtualMachine;
pub use shuru_darwin::VirtualMachineConfiguration;
pub use shuru_darwin::VmState as VirtualMachineState;
pub use shuru_darwin::VzError;
