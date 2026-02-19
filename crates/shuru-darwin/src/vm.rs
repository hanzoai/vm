use std::ffi::c_void;
use std::net::TcpStream;
use std::os::unix::io::FromRawFd;

use block2::ConcreteBlock;
use crossbeam_channel::{bounded, Receiver, Sender};
use objc2::rc::{autoreleasepool, Id, Shared};
use objc2::runtime::{NSObject, NSObjectProtocol, Object};
use objc2::ClassType;
use objc2::{declare_class, msg_send_id};

use crate::configuration::VirtualMachineConfiguration;
use crate::error::{Result, VzError};
use crate::sys::foundation::{NSError, NSKeyValueObservingOptions, NSString};
use crate::sys::queue::{Queue, QueueAttribute};
use crate::sys::virtualization::{
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtualMachine,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
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
    notifier: Sender<VmState>,
    state_notifications: Receiver<VmState>,
}

impl ObserverContext {
    fn state(&self) -> VmState {
        unsafe {
            match self.machine.state() {
                0 => VmState::Stopped,
                1 => VmState::Running,
                2 => VmState::Paused,
                3 => VmState::Error,
                4 => VmState::Starting,
                5 => VmState::Pausing,
                6 => VmState::Resuming,
                7 => VmState::Stopping,
                _ => VmState::Unknown,
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

impl VirtualMachine {
    pub fn new(config: &VirtualMachineConfiguration) -> Self {
        unsafe {
            let queue = Queue::create("com.virt.fwk.rs", QueueAttribute::Serial);
            let machine = VZVirtualMachine::initWithConfiguration_queue(
                VZVirtualMachine::alloc(),
                &config.inner,
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

    pub fn state_channel(&self) -> Receiver<VmState> {
        self.ctx.state_notifications.clone()
    }

    pub fn supported() -> bool {
        unsafe { VZVirtualMachine::isSupported() }
    }

    pub fn start(&self) -> Result<()> {
        unsafe {
            let (tx, rx) = std::sync::mpsc::channel();
            let dispatch_block = ConcreteBlock::new(move || {
                let inner_tx = tx.clone();
                let completion_handler = ConcreteBlock::new(move |err: *mut NSError| {
                    if err.is_null() {
                        inner_tx.send(Ok(())).unwrap();
                    } else {
                        inner_tx
                            .send(Err(VzError::from_ns_error(err.as_mut().unwrap())))
                            .unwrap();
                    }
                });

                let completion_handler = completion_handler.copy();
                self.ctx.machine.startWithCompletionHandler(&completion_handler);
            });

            let dispatch_block_clone = dispatch_block.clone();
            self.queue.exec_block_async(&dispatch_block_clone);

            rx.recv()
                .map_err(|_| VzError::new("VM start channel closed"))?
        }
    }

    pub fn stop(&self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        let dispatch_block = ConcreteBlock::new(move || {
            let inner_tx = tx.clone();
            unsafe {
                let completion_handler = ConcreteBlock::new(move |err: *mut NSError| {
                    if err.is_null() {
                        inner_tx.send(Ok(())).unwrap();
                    } else {
                        inner_tx
                            .send(Err(VzError::from_ns_error(err.as_mut().unwrap())))
                            .unwrap();
                    }
                });

                let completion_handler = completion_handler.copy();
                self.ctx.machine.stopWithCompletionHandler(&completion_handler);
            }
        });

        let dispatch_block_clone = dispatch_block.clone();
        self.queue.exec_block_async(&dispatch_block_clone);

        rx.recv()
            .map_err(|_| VzError::new("VM stop channel closed"))?
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

    /// Connects to a vsock port on the guest and returns a TcpStream.
    /// Must dispatch on the VM's queue per Apple Virtualization framework requirements.
    pub fn connect_to_vsock_port(&self, port: u32) -> Result<TcpStream> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<TcpStream>>();

        let dispatch_block = ConcreteBlock::new(move || {
            let devices = unsafe { self.ctx.machine.socketDevices() };
            let count = devices.count();
            if count == 0 {
                tx.send(Err(VzError::new("No socket devices found on the VM")))
                    .ok();
                return;
            }

            let device: Id<VZVirtioSocketDevice, Shared> =
                unsafe { Id::cast(devices.object_at_index(0)) };

            let inner_tx = tx.clone();
            let completion_handler = ConcreteBlock::new(
                move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                    if !err.is_null() {
                        let error = unsafe { VzError::from_ns_error(&*err) };
                        inner_tx.send(Err(error)).ok();
                    } else if conn.is_null() {
                        inner_tx
                            .send(Err(VzError::new("vsock connection returned null")))
                            .ok();
                    } else {
                        let fd = unsafe { (*conn).fileDescriptor() };
                        // dup the fd so it survives after the connection object is released
                        let duped = unsafe { libc::dup(fd) };
                        if duped < 0 {
                            inner_tx
                                .send(Err(VzError::new("failed to dup vsock fd")))
                                .ok();
                        } else {
                            let stream = unsafe { TcpStream::from_raw_fd(duped) };
                            inner_tx.send(Ok(stream)).ok();
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
            .map_err(|_| VzError::new("vsock connection channel closed"))?
    }

    pub fn state(&self) -> VmState {
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
