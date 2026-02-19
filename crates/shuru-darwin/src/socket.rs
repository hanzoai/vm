use objc2::rc::{Id, Shared};

use crate::sys::virtualization::{
    VZSocketDeviceConfiguration, VZVirtioSocketDeviceConfiguration,
};

pub struct VirtioSocketDevice {
    inner: Id<VZVirtioSocketDeviceConfiguration, Shared>,
}

impl VirtioSocketDevice {
    pub fn new() -> Self {
        VirtioSocketDevice {
            inner: unsafe { VZVirtioSocketDeviceConfiguration::new() },
        }
    }

    pub(crate) fn as_socket_config(&self) -> Id<VZSocketDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}

impl Default for VirtioSocketDevice {
    fn default() -> Self {
        Self::new()
    }
}
