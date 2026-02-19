use objc2::rc::{Id, Shared};

use crate::sys::virtualization::{
    VZEntropyDeviceConfiguration, VZVirtioEntropyDeviceConfiguration,
};

pub struct VirtioEntropyDevice {
    inner: Id<VZVirtioEntropyDeviceConfiguration, Shared>,
}

impl VirtioEntropyDevice {
    pub fn new() -> Self {
        VirtioEntropyDevice {
            inner: unsafe { VZVirtioEntropyDeviceConfiguration::new() },
        }
    }

    pub(crate) fn as_entropy_config(&self) -> Id<VZEntropyDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}

impl Default for VirtioEntropyDevice {
    fn default() -> Self {
        Self::new()
    }
}
