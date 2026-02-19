use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::error::{Result, VzError};
use crate::sys::foundation::{NSString, NSURL};
use crate::sys::virtualization::{
    VZDiskImageStorageDeviceAttachment, VZStorageDeviceConfiguration,
    VZVirtioBlockDeviceConfiguration,
};

pub struct DiskImageAttachment {
    inner: Id<VZDiskImageStorageDeviceAttachment, Shared>,
}

impl DiskImageAttachment {
    pub fn new(path: &str, read_only: bool) -> Result<Self> {
        unsafe {
            let url = NSURL::file_url_with_path(path, false)
                .absolute_url()
                .unwrap();
            let inner = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &url,
                read_only,
            )
            .map_err(|e| VzError::from_ns_error(&e))?;
            Ok(DiskImageAttachment { inner })
        }
    }
}

pub struct VirtioBlockDevice {
    inner: Id<VZVirtioBlockDeviceConfiguration, Shared>,
}

impl VirtioBlockDevice {
    pub fn new(attachment: &DiskImageAttachment) -> Self {
        unsafe {
            let attachment_id: Id<
                crate::sys::virtualization::VZStorageDeviceAttachment,
                Shared,
            > = Id::cast(attachment.inner.clone());
            let inner = VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attachment_id,
            );
            VirtioBlockDevice { inner }
        }
    }

    pub fn validate_identifier(identifier: &str) -> Result<()> {
        unsafe {
            let id = NSString::from_str(identifier);
            VZVirtioBlockDeviceConfiguration::validateBlockDeviceIdentifier_error(&id)
                .map_err(|e| VzError::from_ns_error(&e))?;
            Ok(())
        }
    }

    pub fn set_identifier(&self, identifier: &str) {
        unsafe {
            let id = NSString::from_str(identifier);
            self.inner.setBlockDeviceIdentifier(&id);
        }
    }

    pub(crate) fn as_storage_config(&self) -> Id<VZStorageDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}
