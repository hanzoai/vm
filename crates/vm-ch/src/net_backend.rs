//! vhost-user-net backend bridging cloud-hypervisor's virtio-net device to
//! the proxy's `SOCK_DGRAM` socketpair (one raw Ethernet frame per datagram,
//! same wire format as VZFileHandleNetworkDeviceAttachment on macOS).
//!
//! Queue 0 is RX (host -> guest), queue 1 is TX (guest -> host). Frames carry
//! a 12-byte virtio-net header on the queues and none on the socketpair. A
//! frame arriving while the guest has no RX buffer posted is dropped; the
//! proxy's TCP stack retransmits.

use std::io::Result as IoResult;
use std::os::fd::RawFd;
use std::sync::{Arc, RwLock};

use vhost::vhost_user::message::{VhostUserProtocolFeatures, VhostUserVirtioFeatures};
use vhost::vhost_user::Listener;
use vhost_user_backend::{VhostUserBackendMut, VhostUserDaemon, VringRwLock, VringT};
use virtio_bindings::virtio_config::VIRTIO_F_VERSION_1;
use virtio_bindings::virtio_net::VIRTIO_NET_F_MAC;
use virtio_queue::QueueT;
use vm_memory::{Bytes, GuestAddressSpace, GuestMemoryAtomic, GuestMemoryMmap};
use vmm_sys_util::epoll::EventSet;

const VIRTIO_NET_HDR_SIZE: usize = 12;
const MAX_FRAME: usize = 65536;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;
/// epoll data for the socketpair fd (must be > num_queues).
const SOCK_EVENT: u16 = 3;

struct Net {
    fd: RawFd,
    mem: Option<GuestMemoryAtomic<GuestMemoryMmap>>,
    event_idx: bool,
}

impl Net {
    /// Guest -> host: drain the TX queue, stripping the virtio-net header.
    fn process_tx(&self, vring: &VringRwLock) -> IoResult<()> {
        let Some(atomic_mem) = self.mem.as_ref() else {
            return Ok(());
        };
        let mem = atomic_mem.memory();
        let mut vring = vring.get_mut();
        let mut used = false;
        let mut frame = vec![0u8; MAX_FRAME];

        while let Some(chain) = vring.get_queue_mut().pop_descriptor_chain(mem.clone()) {
            let head = chain.head_index();
            let mut len = 0usize;
            for desc in chain {
                if desc.is_write_only() {
                    continue;
                }
                let n = (desc.len() as usize).min(MAX_FRAME - len);
                if mem
                    .read_slice(&mut frame[len..len + n], desc.addr())
                    .is_err()
                {
                    break;
                }
                len += n;
            }
            if len > VIRTIO_NET_HDR_SIZE {
                unsafe {
                    libc::send(
                        self.fd,
                        frame[VIRTIO_NET_HDR_SIZE..].as_ptr() as *const libc::c_void,
                        len - VIRTIO_NET_HDR_SIZE,
                        libc::MSG_DONTWAIT,
                    );
                }
            }
            vring.add_used(head, 0).ok();
            used = true;
        }
        if used {
            vring.signal_used_queue().ok();
        }
        Ok(())
    }

    /// Host -> guest: drain the socketpair into RX queue buffers, prepending
    /// a zeroed virtio-net header.
    fn process_rx(&self, vring: &VringRwLock) -> IoResult<()> {
        let Some(atomic_mem) = self.mem.as_ref() else {
            return Ok(());
        };
        let mem = atomic_mem.memory();
        let mut vring = vring.get_mut();
        let mut used = false;
        let mut buf = vec![0u8; VIRTIO_NET_HDR_SIZE + MAX_FRAME];

        loop {
            let n = unsafe {
                libc::recv(
                    self.fd,
                    buf[VIRTIO_NET_HDR_SIZE..].as_mut_ptr() as *mut libc::c_void,
                    MAX_FRAME,
                    libc::MSG_DONTWAIT,
                )
            };
            if n <= 0 {
                break; // EAGAIN or closed
            }
            let total = VIRTIO_NET_HDR_SIZE + n as usize;

            let Some(chain) = vring.get_queue_mut().pop_descriptor_chain(mem.clone()) else {
                continue; // no guest buffer posted: drop the frame
            };
            let head = chain.head_index();
            let mut written = 0usize;
            for desc in chain {
                if !desc.is_write_only() || written == total {
                    continue;
                }
                let n = (desc.len() as usize).min(total - written);
                if mem
                    .write_slice(&buf[written..written + n], desc.addr())
                    .is_err()
                {
                    break;
                }
                written += n;
            }
            vring.add_used(head, written as u32).ok();
            used = true;
        }
        if used {
            vring.signal_used_queue().ok();
        }
        Ok(())
    }
}

impl VhostUserBackendMut for Net {
    type Bitmap = ();
    type Vring = VringRwLock;

    fn num_queues(&self) -> usize {
        2
    }

    fn max_queue_size(&self) -> usize {
        256
    }

    fn features(&self) -> u64 {
        (1 << VIRTIO_F_VERSION_1)
            | (1 << VIRTIO_NET_F_MAC)
            | VhostUserVirtioFeatures::PROTOCOL_FEATURES.bits()
    }

    fn protocol_features(&self) -> VhostUserProtocolFeatures {
        VhostUserProtocolFeatures::MQ | VhostUserProtocolFeatures::REPLY_ACK
    }

    fn set_event_idx(&mut self, enabled: bool) {
        self.event_idx = enabled;
    }

    fn update_memory(&mut self, mem: GuestMemoryAtomic<GuestMemoryMmap>) -> IoResult<()> {
        self.mem = Some(mem);
        Ok(())
    }

    fn handle_event(
        &mut self,
        device_event: u16,
        _evset: EventSet,
        vrings: &[VringRwLock],
        _thread_id: usize,
    ) -> IoResult<()> {
        match device_event {
            RX_QUEUE | SOCK_EVENT => self.process_rx(&vrings[RX_QUEUE as usize]),
            TX_QUEUE => self.process_tx(&vrings[TX_QUEUE as usize]),
            _ => Ok(()),
        }
    }
}

/// Serve a vhost-user-net backend on `socket_path` for one connection.
/// Returns after the listener is ready; the daemon runs on its own thread
/// and exits when cloud-hypervisor disconnects.
pub(crate) fn spawn(socket_path: &str, fd: RawFd) -> crate::error::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }

    let backend = Arc::new(RwLock::new(Net {
        fd,
        mem: None,
        event_idx: false,
    }));
    let mut daemon = VhostUserDaemon::new(
        "hanzo-vm-net".to_string(),
        backend,
        GuestMemoryAtomic::new(GuestMemoryMmap::new()),
    )
    .map_err(|e| crate::error::VzError::new(format!("vhost-user-net daemon: {}", e)))?;

    let mut listener = Listener::new(socket_path, true)
        .map_err(|e| crate::error::VzError::new(format!("vhost-user-net listener: {}", e)))?;

    std::thread::Builder::new()
        .name("hanzo-vm-net".into())
        .spawn(move || {
            // start() blocks until cloud-hypervisor connects.
            if daemon.start(&mut listener).is_ok() {
                if let Some(handler) = daemon.get_epoll_handlers().first() {
                    let _ = handler.register_listener(fd, EventSet::IN, SOCK_EVENT as u64);
                }
                let _ = daemon.wait();
            }
        })
        .map_err(|e| crate::error::VzError::new(format!("vhost-user-net thread: {}", e)))?;

    Ok(())
}
