use objc2::rc::{Id, Shared};

use crate::sealed::UnsafeGetId;
use crate::sys::virtualization::{
    VZSocketDeviceConfiguration, VZVirtioSocketDeviceConfiguration,
};

pub trait SocketDeviceConfiguration: UnsafeGetId<VZSocketDeviceConfiguration> {}

#[derive(Debug)]
pub struct VirtioSocketDeviceConfiguration {
    inner: Id<VZVirtioSocketDeviceConfiguration, Shared>,
}

impl VirtioSocketDeviceConfiguration {
    pub fn new() -> Self {
        unsafe {
            VirtioSocketDeviceConfiguration {
                inner: VZVirtioSocketDeviceConfiguration::new(),
            }
        }
    }
}

impl SocketDeviceConfiguration for VirtioSocketDeviceConfiguration {}

impl Default for VirtioSocketDeviceConfiguration {
    fn default() -> Self {
        Self::new()
    }
}

impl UnsafeGetId<VZSocketDeviceConfiguration> for VirtioSocketDeviceConfiguration {
    fn id(&self) -> Id<VZSocketDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}
