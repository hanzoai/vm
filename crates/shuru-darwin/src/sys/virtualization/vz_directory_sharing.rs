#![allow(non_snake_case)]
use objc2::rc::{Allocated, Id, Shared};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{extern_class, extern_methods, ClassType};

use crate::sys::foundation::*;

// region: VZDirectorySharingDeviceConfiguration
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZDirectorySharingDeviceConfiguration;

    unsafe impl ClassType for VZDirectorySharingDeviceConfiguration {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZDirectorySharingDeviceConfiguration {}

extern_methods!(
    unsafe impl VZDirectorySharingDeviceConfiguration {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Id<Self, Shared>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Option<Allocated<Self>>) -> Id<Self, Shared>;
    }
);
// endregion

// region: VZVirtioFileSystemDeviceConfiguration
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub(crate) struct VZVirtioFileSystemDeviceConfiguration;

    unsafe impl ClassType for VZVirtioFileSystemDeviceConfiguration {
        #[inherits(NSObject)]
        type Super = VZDirectorySharingDeviceConfiguration;
    }
);

unsafe impl NSObjectProtocol for VZVirtioFileSystemDeviceConfiguration {}

extern_methods!(
    unsafe impl VZVirtioFileSystemDeviceConfiguration {
        #[method_id(@__retain_semantics Init initWithTag:)]
        pub unsafe fn initWithTag(
            this: Option<Allocated<Self>>,
            tag: &NSString,
        ) -> Id<Self, Shared>;

        #[method_id(@__retain_semantics Other share)]
        pub unsafe fn share(&self) -> Option<Id<VZDirectoryShare, Shared>>;

        #[method(setShare:)]
        pub unsafe fn setShare(&self, share: &VZDirectoryShare);
    }
);
// endregion

// region: VZDirectoryShare
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZDirectoryShare;

    unsafe impl ClassType for VZDirectoryShare {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZDirectoryShare {}

extern_methods!(
    unsafe impl VZDirectoryShare {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Id<Self, Shared>;

        #[method_id(@__retain_semantics Init init)]
        pub unsafe fn init(this: Option<Allocated<Self>>) -> Id<Self, Shared>;
    }
);
// endregion

// region: VZSingleDirectoryShare
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub(crate) struct VZSingleDirectoryShare;

    unsafe impl ClassType for VZSingleDirectoryShare {
        #[inherits(NSObject)]
        type Super = VZDirectoryShare;
    }
);

unsafe impl NSObjectProtocol for VZSingleDirectoryShare {}

extern_methods!(
    unsafe impl VZSingleDirectoryShare {
        #[method_id(@__retain_semantics Init initWithDirectory:)]
        pub unsafe fn initWithDirectory(
            this: Option<Allocated<Self>>,
            directory: &VZSharedDirectory,
        ) -> Id<Self, Shared>;
    }
);
// endregion

// region: VZSharedDirectory
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub(crate) struct VZSharedDirectory;

    unsafe impl ClassType for VZSharedDirectory {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZSharedDirectory {}

extern_methods!(
    unsafe impl VZSharedDirectory {
        #[method_id(@__retain_semantics Init initWithURL:readOnly:)]
        pub unsafe fn initWithURL_readOnly(
            this: Option<Allocated<Self>>,
            url: &NSURL,
            read_only: bool,
        ) -> Id<Self, Shared>;
    }
);
// endregion
