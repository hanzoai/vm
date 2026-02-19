use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::sys::virtualization::{
    VZMemoryBalloonDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
};

pub struct VirtioMemoryBalloonDevice {
    inner: Id<VZVirtioTraditionalMemoryBalloonDeviceConfiguration, Shared>,
}

impl VirtioMemoryBalloonDevice {
    pub fn new() -> Self {
        VirtioMemoryBalloonDevice {
            inner: unsafe {
                VZVirtioTraditionalMemoryBalloonDeviceConfiguration::init(
                    VZVirtioTraditionalMemoryBalloonDeviceConfiguration::alloc(),
                )
            },
        }
    }

    pub(crate) fn as_memory_balloon_config(
        &self,
    ) -> Id<VZMemoryBalloonDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}

impl Default for VirtioMemoryBalloonDevice {
    fn default() -> Self {
        Self::new()
    }
}
