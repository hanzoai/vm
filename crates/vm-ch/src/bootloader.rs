use std::cell::RefCell;

/// Stores the kernel, initrd, and command-line paths for booting a Linux guest.
/// On macOS this wraps VZLinuxBootLoader; here it is a plain data holder
/// consumed by `VirtualMachine::new()`.
///
/// Uses `RefCell` for interior mutability to match Darwin`s `&self` setter API.
pub struct LinuxBootLoader {
    pub(crate) kernel_path: String,
    pub(crate) initrd_path: RefCell<Option<String>>,
    pub(crate) command_line: RefCell<Option<String>>,
}

impl LinuxBootLoader {
    pub fn new(kernel_path: &str, initrd_path: &str, command_line: &str) -> Self {
        let bl = Self::new_with_kernel(kernel_path);
        bl.set_initrd(initrd_path);
        bl.set_command_line(command_line);
        bl
    }

    pub fn new_with_kernel(kernel_path: &str) -> Self {
        LinuxBootLoader {
            kernel_path: kernel_path.to_string(),
            initrd_path: RefCell::new(None),
            command_line: RefCell::new(None),
        }
    }

    pub fn set_initrd(&self, initrd_path: &str) {
        *self.initrd_path.borrow_mut() = Some(initrd_path.to_string());
    }

    /// cloud-hypervisor attaches a virtio-console, so `console=hvc0` from the
    /// caller is kept as-is.
    pub fn set_command_line(&self, command_line: &str) {
        *self.command_line.borrow_mut() = Some(command_line.to_string());
    }
}
