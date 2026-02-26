use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::sys::foundation::{NSString, NSURL};
use crate::sys::virtualization::{
    VZDirectoryShare, VZDirectorySharingDeviceConfiguration, VZSharedDirectory,
    VZSingleDirectoryShare, VZVirtioFileSystemDeviceConfiguration,
};

pub struct SharedDirectory {
    inner: Id<VZSharedDirectory, Shared>,
}

impl SharedDirectory {
    pub fn new(path: &str, read_only: bool) -> Self {
        unsafe {
            let url = NSURL::file_url_with_path(path, true);
            let inner =
                VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &url, read_only);
            SharedDirectory { inner }
        }
    }
}

pub struct VirtioFileSystemDevice {
    inner: Id<VZVirtioFileSystemDeviceConfiguration, Shared>,
}

impl VirtioFileSystemDevice {
    pub fn new(tag: &str, directory: &SharedDirectory) -> Self {
        unsafe {
            let ns_tag = NSString::from_str(tag);
            let inner = VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &ns_tag,
            );

            let single_share: Id<VZDirectoryShare, Shared> = Id::cast(
                VZSingleDirectoryShare::initWithDirectory(
                    VZSingleDirectoryShare::alloc(),
                    &directory.inner,
                ),
            );
            inner.setShare(&single_share);

            VirtioFileSystemDevice { inner }
        }
    }

    pub(crate) fn as_directory_sharing_config(
        &self,
    ) -> Id<VZDirectorySharingDeviceConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}
