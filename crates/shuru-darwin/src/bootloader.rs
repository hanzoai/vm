use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::sys::foundation::{NSString, NSURL};
use crate::sys::virtualization::{VZBootLoader, VZLinuxBootLoader};

pub struct LinuxBootLoader {
    inner: Id<VZLinuxBootLoader, Shared>,
}

impl LinuxBootLoader {
    pub fn new(kernel_path: &str, initrd_path: &str, command_line: &str) -> Self {
        let boot_loader = Self::new_with_kernel(kernel_path);
        boot_loader.set_initrd(initrd_path);
        boot_loader.set_command_line(command_line);
        boot_loader
    }

    pub fn new_with_kernel(kernel_path: &str) -> Self {
        unsafe {
            let kernel_url = NSURL::file_url_with_path(kernel_path, false)
                .absolute_url()
                .unwrap();
            LinuxBootLoader {
                inner: VZLinuxBootLoader::initWithKernelURL(
                    VZLinuxBootLoader::alloc(),
                    &kernel_url,
                ),
            }
        }
    }

    pub fn set_initrd(&self, initrd_path: &str) {
        unsafe {
            let initrd_url = NSURL::file_url_with_path(initrd_path, false)
                .absolute_url()
                .unwrap();
            self.inner.setInitialRamdiskURL(Some(&initrd_url));
        }
    }

    pub fn set_command_line(&self, command_line: &str) {
        unsafe {
            let command_line = NSString::from_str(command_line);
            self.inner.setCommandLine(&command_line);
        }
    }

    pub(crate) fn as_vz_boot_loader(&self) -> Id<VZBootLoader, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}
