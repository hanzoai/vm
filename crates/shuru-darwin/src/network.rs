use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::sys::virtualization::{
    VZMACAddress, VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration,
};

pub struct NATNetworkAttachment {
    inner: Id<VZNATNetworkDeviceAttachment, Shared>,
}

impl NATNetworkAttachment {
    pub fn new() -> Self {
        NATNetworkAttachment {
            inner: unsafe {
                VZNATNetworkDeviceAttachment::init(VZNATNetworkDeviceAttachment::alloc())
            },
        }
    }
}

impl Default for NATNetworkAttachment {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MACAddress {
    inner: Id<VZMACAddress, Shared>,
}

impl MACAddress {
    pub fn new() -> Self {
        MACAddress {
            inner: unsafe { VZMACAddress::init(VZMACAddress::alloc()) },
        }
    }

    pub fn random_local() -> Self {
        MACAddress {
            inner: unsafe { VZMACAddress::randomLocallyAdministeredAddress() },
        }
    }
}

impl Default for MACAddress {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VirtioNetworkDevice {
    inner: Id<VZVirtioNetworkDeviceConfiguration, Shared>,
}

impl VirtioNetworkDevice {
    pub fn new() -> Self {
        VirtioNetworkDevice {
            inner: unsafe {
                VZVirtioNetworkDeviceConfiguration::init(
                    VZVirtioNetworkDeviceConfiguration::alloc(),
                )
            },
        }
    }

    pub fn new_with_attachment(attachment: &NATNetworkAttachment) -> Self {
        let config = Self::new();
        config.set_attachment(attachment);
        config
    }

    pub fn set_attachment(&self, attachment: &NATNetworkAttachment) {
        unsafe {
            let id: Id<crate::sys::virtualization::VZNetworkDeviceAttachment, Shared> =
                Id::cast(attachment.inner.clone());
            self.inner.setAttachment(Some(&id));
        }
    }

    pub fn set_mac_address(&self, address: &MACAddress) {
        unsafe {
            self.inner.setMACAddress(&address.inner);
        }
    }

    pub(crate) fn as_network_config(&self) -> Id<VZNetworkDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}

impl Default for VirtioNetworkDevice {
    fn default() -> Self {
        Self::new()
    }
}
