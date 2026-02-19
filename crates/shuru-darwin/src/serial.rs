use std::os::fd::RawFd;

use objc2::rc::{Id, Shared};
use objc2::ClassType;

use crate::sys::foundation::NSFileHandle;
use crate::sys::virtualization::{
    VZFileHandleSerialPortAttachment, VZSerialPortConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration,
};

pub struct FileHandleSerialAttachment {
    inner: Id<VZFileHandleSerialPortAttachment, Shared>,
}

impl FileHandleSerialAttachment {
    pub fn new(read_fd: RawFd, write_fd: RawFd) -> Self {
        unsafe {
            let file_handle_for_reading =
                NSFileHandle::initWithFileDescriptor(NSFileHandle::alloc(), read_fd);
            let file_handle_for_writing =
                NSFileHandle::initWithFileDescriptor(NSFileHandle::alloc(), write_fd);

            let attachment =
                VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                    VZFileHandleSerialPortAttachment::alloc(),
                    Some(&file_handle_for_reading),
                    Some(&file_handle_for_writing),
                );
            FileHandleSerialAttachment { inner: attachment }
        }
    }
}

pub struct VirtioConsoleSerialPort {
    inner: Id<VZVirtioConsoleDeviceSerialPortConfiguration, Shared>,
}

impl VirtioConsoleSerialPort {
    pub fn new() -> Self {
        VirtioConsoleSerialPort {
            inner: VZVirtioConsoleDeviceSerialPortConfiguration::new(),
        }
    }

    pub fn new_with_attachment(attachment: &FileHandleSerialAttachment) -> Self {
        let config = Self::new();
        config.set_attachment(attachment);
        config
    }

    pub fn set_attachment(&self, attachment: &FileHandleSerialAttachment) {
        unsafe {
            let id: Id<crate::sys::virtualization::VZSerialPortAttachment, Shared> =
                Id::cast(attachment.inner.clone());
            self.inner.setAttachment(Some(&id));
        }
    }

    pub(crate) fn as_serial_port_config(&self) -> Id<VZSerialPortConfiguration, Shared> {
        unsafe { Id::cast(self.inner.clone()) }
    }
}

impl Default for VirtioConsoleSerialPort {
    fn default() -> Self {
        Self::new()
    }
}
