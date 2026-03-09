use std::os::unix::io::RawFd;

use smoltcp::phy::{self, Checksum, Device, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

const MTU: usize = 1514; // 14-byte Ethernet header + 1500-byte IP payload

/// smoltcp Device backed by a Unix datagram socketpair fd.
///
/// One end of the socketpair is given to VZFileHandleNetworkDeviceAttachment
/// (the VM side). This Device reads/writes the other end (the host side),
/// giving us raw L2 Ethernet frames from/to the guest.
pub struct VZDevice {
    fd: RawFd,
    recv_buf: Vec<u8>,
    /// Frame pre-read by `try_recv()`, waiting to be consumed by smoltcp.
    pending_rx: Option<Vec<u8>>,
}

impl VZDevice {
    pub fn new(fd: RawFd) -> Self {
        VZDevice {
            fd,
            recv_buf: vec![0u8; MTU + 64], // slack for oversized frames
            pending_rx: None,
        }
    }

    /// Attempt to read a frame from the socketpair (non-blocking).
    /// Call this before `Interface::poll()` so we can inspect the frame
    /// (e.g. to detect TCP SYN and dynamically add listening sockets).
    pub fn try_recv(&mut self) {
        if self.pending_rx.is_some() {
            return; // already have a pending frame
        }

        let n = unsafe {
            libc::recv(
                self.fd,
                self.recv_buf.as_mut_ptr() as *mut libc::c_void,
                self.recv_buf.len(),
                libc::MSG_DONTWAIT,
            )
        };

        if n > 0 {
            self.pending_rx = Some(self.recv_buf[..n as usize].to_vec());
        }
    }

    /// Peek at the pending frame without consuming it.
    pub fn peek_frame(&self) -> Option<&[u8]> {
        self.pending_rx.as_deref()
    }
}

pub struct VZRxToken {
    buffer: Vec<u8>,
}

impl phy::RxToken for VZRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.buffer)
    }
}

pub struct VZTxToken {
    fd: RawFd,
}

impl phy::TxToken for VZTxToken {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = vec![0u8; len];
        let result = f(&mut buffer);
        let sent = unsafe {
            libc::send(
                self.fd,
                buffer.as_ptr() as *const libc::c_void,
                buffer.len(),
                0,
            )
        };
        if sent < 0 {
            tracing::warn!("TX {len} bytes failed: sent={sent}");
        }
        result
    }
}

impl Device for VZDevice {
    type RxToken<'a> = VZRxToken;
    type TxToken<'a> = VZTxToken;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let buffer = self.pending_rx.take()?;
        Some((VZRxToken { buffer }, VZTxToken { fd: self.fd }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(VZTxToken { fd: self.fd })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = MTU;
        // The guest VirtIO NIC offloads checksum calculation, so incoming
        // frames may have partial/invalid checksums. Tell smoltcp to only
        // generate checksums on TX (for the guest to verify), not verify on RX.
        caps.checksum.ipv4 = Checksum::Tx;
        caps.checksum.tcp = Checksum::Tx;
        caps.checksum.udp = Checksum::Tx;
        caps.checksum.icmpv4 = Checksum::Tx;
        caps
    }
}
