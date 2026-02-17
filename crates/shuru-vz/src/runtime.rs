use std::ffi::c_void;

use block2::ConcreteBlock;
use crossbeam_channel::{bounded, Receiver, Sender};
use objc2::rc::{autoreleasepool, Id, Shared};
use objc2::runtime::{NSObject, NSObjectProtocol, Object};
use objc2::ClassType;
use objc2::{declare_class, msg_send_id};

use crate::sealed::UnsafeGetId;
use crate::sys::foundation::{NSError, NSKeyValueObservingOptions, NSString};
use crate::sys::virtualization::{
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtualMachine,
};

use crate::configuration::VirtualMachineConfiguration;
use crate::sys::queue::{Queue, QueueAttribute};

#[derive(Debug)]
pub enum VirtualMachineState {
    Stopped = 0,
    Running = 1,
    Paused = 2,
    Error = 3,
    Starting = 4,
    Pausing = 5,
    Resuming = 6,
    Stopping = 7,
    Unknown = -1,
}

/// Heap-allocated context for the KVO observer.
/// Must live on the heap so the pointer remains stable after VirtualMachine moves.
#[derive(Debug)]
struct ObserverContext {
    machine: Id<VZVirtualMachine, Shared>,
    notifier: Sender<VirtualMachineState>,
    state_notifications: Receiver<VirtualMachineState>,
}

impl ObserverContext {
    fn state(&self) -> VirtualMachineState {
        unsafe {
            match self.machine.state() {
                0 => VirtualMachineState::Stopped,
                1 => VirtualMachineState::Running,
                2 => VirtualMachineState::Paused,
                3 => VirtualMachineState::Error,
                4 => VirtualMachineState::Starting,
                5 => VirtualMachineState::Pausing,
                6 => VirtualMachineState::Resuming,
                7 => VirtualMachineState::Stopping,
                _ => VirtualMachineState::Unknown,
            }
        }
    }
}

#[derive(Debug)]
pub struct VirtualMachine {
    ctx: Box<ObserverContext>,
    queue: Queue,
    observer: Id<VirtualMachineStateObserver, Shared>,
}

/// VirtualMachine represents the entire state of a single virtual machine.
///
/// **Support**: macOS 11.0+
///
/// Creating a virtual machine using the Virtualization framework requires the app to have the "com.apple.security.virtualization" entitlement.
/// see: <https://developer.apple.com/documentation/virtualization/vzvirtualmachine?language=objc>
impl VirtualMachine {
    pub fn new(config: &VirtualMachineConfiguration) -> Self {
        unsafe {
            let queue = Queue::create("com.virt.fwk.rs", QueueAttribute::Serial);
            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config.id(),
                queue.ptr,
            );

            let (sender, receiver) = bounded(1);
            let observer = VirtualMachineStateObserver::new();

            let ctx = Box::new(ObserverContext {
                machine,
                notifier: sender,
                state_notifications: receiver,
            });

            // Use the Box's stable heap address as KVO context
            let ctx_ptr: *const ObserverContext = &*ctx;

            let key = NSString::from_str("state");
            ctx.machine.addObserver_forKeyPath_options_context(
                &observer,
                &key,
                NSKeyValueObservingOptions::NSKeyValueObservingOptionNew,
                ctx_ptr as *mut c_void,
            );

            VirtualMachine {
                ctx,
                queue,
                observer,
            }
        }
    }

    /// Returns a crossbeam receiver channel for VM state changes.
    pub fn get_state_channel(&self) -> Receiver<VirtualMachineState> {
        self.ctx.state_notifications.clone()
    }

    /// Returns whether the system supports virtualization.
    pub fn supported() -> bool {
        unsafe { VZVirtualMachine::isSupported() }
    }

    /// Synchronously starts the VirtualMachine.
    pub fn start(&self) -> Result<(), String> {
        unsafe {
            let (tx, rx) = std::sync::mpsc::channel();
            let dispatch_block = ConcreteBlock::new(move || {
                let inner_tx = tx.clone();
                let completion_handler = ConcreteBlock::new(move |err: *mut NSError| {
                    if err.is_null() {
                        inner_tx.send(Ok(())).unwrap();
                    } else {
                        inner_tx
                            .send(Err(err.as_mut().unwrap().localized_description()))
                            .unwrap();
                    }
                });

                let completion_handler = completion_handler.copy();
                self.ctx.machine.startWithCompletionHandler(&completion_handler);
            });

            let dispatch_block_clone = dispatch_block.clone();
            self.queue.exec_block_async(&dispatch_block_clone);

            let result = rx.recv();

            if result.is_err() {
                return Err("TODO: implement better error handling here!".into());
            }

            result.unwrap()?;

            Ok(())
        }
    }

    /// Synchronously stops the VirtualMachine.
    pub fn stop(&self) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let dispatch_block = ConcreteBlock::new(move || {
            let inner_tx = tx.clone();
            unsafe {
                let completion_handler = ConcreteBlock::new(move |err: *mut NSError| {
                    if err.is_null() {
                        inner_tx.send(Ok(())).unwrap();
                    } else {
                        inner_tx
                            .send(Err(err.as_mut().unwrap().localized_description()))
                            .unwrap();
                    }
                });

                let completion_handler = completion_handler.copy();
                self.ctx.machine.stopWithCompletionHandler(&completion_handler);
            }
        });

        let dispatch_block_clone = dispatch_block.clone();
        self.queue.exec_block_async(&dispatch_block_clone);

        let result = rx.recv();

        if result.is_err() {
            return Err("TODO: implement better error handling here!".into());
        }

        result.unwrap()?;

        Ok(())
    }

    pub fn can_start(&self) -> bool {
        self.queue
            .exec_sync(move || unsafe { self.ctx.machine.canStart() })
    }

    pub fn can_stop(&self) -> bool {
        self.queue
            .exec_sync(move || unsafe { self.ctx.machine.canRequestStop() })
    }

    pub fn can_pause(&self) -> bool {
        self.queue
            .exec_sync(move || unsafe { self.ctx.machine.canPause() })
    }

    pub fn can_resume(&self) -> bool {
        self.queue
            .exec_sync(move || unsafe { self.ctx.machine.canResume() })
    }

    pub fn can_request_stop(&self) -> bool {
        self.queue
            .exec_sync(move || unsafe { self.ctx.machine.canRequestStop() })
    }

    /// Returns the list of socket devices configured on this VM.
    pub fn socket_devices(&self) -> Vec<Id<VZVirtioSocketDevice, Shared>> {
        self.queue.exec_sync(move || unsafe {
            let devices = self.ctx.machine.socketDevices();
            let count = devices.count();
            let mut result = Vec::with_capacity(count);
            for i in 0..count {
                let device = devices.object_at_index(i);
                result.push(Id::cast(device));
            }
            result
        })
    }

    /// Connects to a vsock port on the guest and returns the raw file descriptor.
    /// Must dispatch on the VM's queue per Apple Virtualization framework requirements.
    pub fn connect_to_vsock_port(&self, port: u32) -> Result<i32, String> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<i32, String>>();

        let dispatch_block = ConcreteBlock::new(move || {
            let devices = unsafe { self.ctx.machine.socketDevices() };
            let count = unsafe { devices.count() };
            if count == 0 {
                tx.send(Err("No socket devices found on the VM".to_string())).ok();
                return;
            }

            let device: Id<VZVirtioSocketDevice, Shared> =
                unsafe { Id::cast(devices.object_at_index(0)) };

            let inner_tx = tx.clone();
            let completion_handler = ConcreteBlock::new(
                move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                    if !err.is_null() {
                        let msg = unsafe { (*err).localized_description() };
                        inner_tx.send(Err(msg)).ok();
                    } else if conn.is_null() {
                        inner_tx
                            .send(Err("vsock connection returned null".to_string()))
                            .ok();
                    } else {
                        let fd = unsafe { (*conn).fileDescriptor() };
                        // dup the fd so it survives after the connection object is released
                        let duped = unsafe { libc::dup(fd) };
                        if duped < 0 {
                            inner_tx
                                .send(Err("failed to dup vsock fd".to_string()))
                                .ok();
                        } else {
                            inner_tx.send(Ok(duped)).ok();
                        }
                    }
                },
            );
            let completion_handler = completion_handler.copy();

            unsafe {
                device.connectToPort_completionHandler(port, &completion_handler);
            }
        });

        let dispatch_block_clone = dispatch_block.clone();
        self.queue.exec_block_async(&dispatch_block_clone);

        rx.recv()
            .map_err(|_| "vsock connection channel closed".to_string())?
    }

    /// Returns the current execution state of the VM.
    pub fn state(&self) -> VirtualMachineState {
        self.ctx.state()
    }
}

impl Drop for VirtualMachine {
    fn drop(&mut self) {
        let key_path = NSString::from_str("state");
        let ctx_ptr: *const ObserverContext = &*self.ctx;

        unsafe {
            self.ctx.machine.removeObserver_forKeyPath_context(
                &self.observer,
                &key_path,
                ctx_ptr as *mut c_void,
            );
        }
    }
}

declare_class!(
    #[derive(Debug)]
    struct VirtualMachineStateObserver;

    unsafe impl ClassType for VirtualMachineStateObserver {
        type Super = NSObject;
        const NAME: &'static str = "VirtualMachineStateObserver";
    }

    unsafe impl VirtualMachineStateObserver {
        #[method(observeValueForKeyPath:ofObject:change:context:)]
        unsafe fn observe_value_for_key_path(
            &self,
            key_path: Option<&NSString>,
            _object: Option<&NSObject>,
            _change: Option<&Object>,
            context: *mut c_void,
        ) {
            if let Some(msg) = key_path {
                let key = autoreleasepool(|pool| msg.as_str(pool).to_owned());

                if key == "state" {
                    let ctx: &ObserverContext = &*(context as *const ObserverContext);
                    let _ = ctx.state_notifications.try_recv();
                    let _ = ctx.notifier.send(ctx.state());
                }
            }
        }
    }
);

unsafe impl NSObjectProtocol for VirtualMachineStateObserver {}

unsafe impl Send for VirtualMachineStateObserver {}

unsafe impl Sync for VirtualMachineStateObserver {}

impl VirtualMachineStateObserver {
    pub fn new() -> Id<Self, Shared> {
        unsafe { msg_send_id![Self::alloc(), init] }
    }
}
