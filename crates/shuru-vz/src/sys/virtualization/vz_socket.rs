#![allow(non_snake_case)]
use objc2::rc::{Id, Shared};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{extern_class, extern_methods, ClassType};

// region: VZSocketDeviceConfiguration
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZSocketDeviceConfiguration;

    unsafe impl ClassType for VZSocketDeviceConfiguration {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZSocketDeviceConfiguration {}
// endregion

// region: VZVirtioSocketDeviceConfiguration
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub(crate) struct VZVirtioSocketDeviceConfiguration;

    unsafe impl ClassType for VZVirtioSocketDeviceConfiguration {
        #[inherits(NSObject)]
        type Super = VZSocketDeviceConfiguration;
    }
);

unsafe impl NSObjectProtocol for VZVirtioSocketDeviceConfiguration {}

extern_methods!(
    unsafe impl VZVirtioSocketDeviceConfiguration {
        #[method_id(@__retain_semantics New new)]
        pub unsafe fn new() -> Id<Self, Shared>;
    }
);
// endregion

// region: VZSocketDevice
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZSocketDevice;

    unsafe impl ClassType for VZSocketDevice {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZSocketDevice {}
unsafe impl Send for VZSocketDevice {}
unsafe impl Sync for VZSocketDevice {}
// endregion

// region: VZVirtioSocketDevice
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtioSocketDevice;

    unsafe impl ClassType for VZVirtioSocketDevice {
        #[inherits(NSObject)]
        type Super = VZSocketDevice;
    }
);

unsafe impl NSObjectProtocol for VZVirtioSocketDevice {}
unsafe impl Send for VZVirtioSocketDevice {}
unsafe impl Sync for VZVirtioSocketDevice {}

extern_methods!(
    unsafe impl VZVirtioSocketDevice {
        #[method(connectToPort:completionHandler:)]
        pub unsafe fn connectToPort_completionHandler(
            &self,
            port: u32,
            completion_handler: &block2::Block<(*mut VZVirtioSocketConnection, *mut crate::sys::foundation::NSError), ()>,
        );
    }
);
// endregion

// region: VZVirtioSocketConnection
extern_class!(
    #[derive(Debug, PartialEq, Eq, Hash)]
    pub struct VZVirtioSocketConnection;

    unsafe impl ClassType for VZVirtioSocketConnection {
        type Super = NSObject;
    }
);

unsafe impl NSObjectProtocol for VZVirtioSocketConnection {}

extern_methods!(
    unsafe impl VZVirtioSocketConnection {
        #[method(fileDescriptor)]
        pub unsafe fn fileDescriptor(&self) -> i32;

        #[method(sourcePort)]
        pub unsafe fn sourcePort(&self) -> u32;

        #[method(destinationPort)]
        pub unsafe fn destinationPort(&self) -> u32;
    }
);
// endregion
