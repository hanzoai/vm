//! Binding a measurement to hardware.
//!
//! A measurement computed by a launcher says what the launcher loaded. It is
//! reproducible, and it is worth exactly as much as the machine that produced
//! it. To turn it into evidence, the same 64 bytes have to come back out of a
//! chip that the host cannot impersonate.
//!
//! Both shipping confidential-computing platforms offer the same primitive: a
//! guest asks its firmware for a report over 64 caller-supplied bytes, and the
//! report also carries the platform's OWN measurement of how that guest was
//! started. Put [`Measurement::bind`](crate::Measurement::bind) in the caller
//! field and a verifier gets two facts in one signature: this guest holds this
//! measurement log, and this guest was launched from this image. The first is
//! what a log alone cannot establish; the second is what a log cannot even
//! observe.
//!
//! The request is made from INSIDE the guest — `/dev/sev-guest` and
//! `/dev/tdx_guest` are guest devices, present only when the guest is running
//! under SEV-SNP or TDX. So [`status`] is called by `vm-guest`, which reaches
//! it over vsock: the host sends the 64 bytes as an `ATTEST_REQ` frame and the
//! guest answers with a [`Status`]. Asking the HOST would answer a different
//! question — whether the machine running the launcher is itself somebody's
//! confidential guest — and a report over our bind from there says nothing
//! about the VM we started.
//!
//! What is implemented here: platform detection, the two request ABIs, and the
//! ioctls that carry them. What is NOT exercised: the ioctls themselves, for
//! want of the hardware — see this module's tests, which pin the structure
//! sizes and ioctl encodings against the published kernel headers, and the
//! specification for the parts a verifier still has to supply (the VCEK or
//! quote chain, and an expected launch digest to compare the platform's own
//! measurement against).
//!
//! Hardware that would exercise it: AMD EPYC (Milan or later) with SEV-SNP
//! enabled in firmware and a VMM that starts SNP guests, or Intel Xeon with
//! TDX enabled and a TDX-aware VMM. AMD's client parts, Apple silicon and
//! NVIDIA's Grace-Blackwell workstation parts offer no equivalent to a
//! third-party VMM; none of the three machines this was built on can run the
//! path, so on all three [`platform`] answers `None` and a document records
//! `none`.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// A confidential-computing platform that will sign a report for its guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    /// AMD SEV-SNP. The report is signed by the VCEK and is verifiable off the
    /// machine against AMD's key distribution service.
    SevSnp,
    /// Intel TDX. The ioctl returns a TDREPORT — locally verifiable by the
    /// quoting enclave, which converts it into a remotely verifiable quote.
    Tdx,
}

impl Platform {
    pub fn device(self) -> &'static str {
        match self {
            Platform::SevSnp => "/dev/sev-guest",
            Platform::Tdx => "/dev/tdx_guest",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Platform::SevSnp => "sev-snp",
            Platform::Tdx => "tdx",
        }
    }
}

/// Which platform this machine can produce a report from, if any. Presence of
/// the guest device is the whole test: the driver binds only inside a guest
/// the platform actually protects.
pub fn platform() -> Option<Platform> {
    [Platform::SevSnp, Platform::Tdx]
        .into_iter()
        .find(|p| std::path::Path::new(p.device()).exists())
}

/// What a measurement document records about the machine it was made on:
/// which platform signed for it, and the report, if any.
///
/// A report is an OBSERVATION attached to a measurement, never part of it. The
/// check a verifier owes it is one line: the report's REPORT_DATA field must
/// equal the measurement's bind. A report whose caller field says something
/// else is a report about a different measurement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// `none`, `sev-snp` or `tdx`.
    pub platform: String,
    /// The raw report, hex-encoded, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

impl Default for Status {
    fn default() -> Status {
        Status {
            platform: "none".to_string(),
            report: None,
        }
    }
}

/// Ask this machine's platform for a report over `bind`, when it has one. A
/// device that will not answer is not a platform we can claim: the status
/// falls back to `none` rather than asserting hardware that produced nothing.
pub fn status(bind: &[u8; 64]) -> Status {
    match platform().map(|p| (p, report(p, bind))) {
        Some((p, Ok(bytes))) => Status {
            platform: p.label().to_string(),
            report: Some(bytes.iter().map(|b| format!("{b:02x}")).collect()),
        },
        Some((_, Err(_))) | None => Status::default(),
    }
}

/// The raw report bytes: an SEV-SNP attestation report (1184 bytes) or a TDX
/// TDREPORT (1024 bytes), exactly as the platform produced them. Parsing them
/// belongs to the verifier, not to the thing that asked.
pub fn report(platform: Platform, bind: &[u8; 64]) -> Result<Vec<u8>> {
    match platform {
        Platform::SevSnp => snp_report(bind),
        Platform::Tdx => tdx_report(bind),
    }
}

/// The kernel ABIs — `linux/sev-guest.h` and `linux/tdx-guest.h`.
///
/// Compiled wherever the tests run, not only where the call can be made: an
/// ioctl number encodes the size of the structure it carries, so a structure
/// that drifts sends a number no driver answers. That is the one part of this
/// path a machine without the hardware can still check.
#[cfg(any(target_os = "linux", test))]
mod abi {
    /// `struct snp_report_req`: the 64 bytes to sign, the VMPL to sign at.
    #[repr(C)]
    pub struct SnpRequest {
        pub user_data: [u8; 64],
        pub vmpl: u32,
        pub reserved: [u8; 28],
    }

    /// `struct snp_guest_request_ioctl`: the message version, addresses of the
    /// request and response buffers, and the firmware's error on failure.
    #[repr(C)]
    pub struct SnpMessage {
        pub version: u8,
        pub request: u64,
        pub response: u64,
        pub error: u64,
    }

    /// `struct snp_report_resp`: a fixed 4000-byte buffer. The attestation
    /// report begins after the status word, its length and the reserved bytes.
    pub const SNP_RESPONSE: usize = 4000;
    pub const SNP_REPORT_OFFSET: usize = 32;
    pub const SNP_REPORT_LENGTH: usize = 1184;

    pub const TDX_REPORT_LENGTH: usize = 1024;

    /// `struct tdx_report_req`: the 64 bytes to sign, and the buffer the
    /// TDREPORT lands in.
    #[repr(C)]
    pub struct TdxRequest {
        pub report_data: [u8; 64],
        pub report: [u8; TDX_REPORT_LENGTH],
    }

    /// `_IOWR(type, nr, size)` — the encoding both numbers below are built
    /// from, so neither is a constant copied out of a header. An ioctl number
    /// is 32 bits wide; how it is then passed is the platform's business
    /// (glibc takes an unsigned long, musl a signed int), so the width is
    /// fixed here and the cast is made at the call.
    const fn iowr(kind: u8, nr: u8, size: usize) -> u32 {
        const READ_WRITE: u32 = 3;
        (READ_WRITE << 30) | ((size as u32) << 16) | ((kind as u32) << 8) | nr as u32
    }

    /// `SNP_GET_REPORT = _IOWR('S', 0x0, struct snp_guest_request_ioctl)`
    pub const SNP_GET_REPORT: u32 = iowr(b'S', 0, std::mem::size_of::<SnpMessage>());

    /// `TDX_CMD_GET_REPORT0 = _IOWR('T', 1, struct tdx_report_req)`
    pub const TDX_GET_REPORT: u32 = iowr(b'T', 1, std::mem::size_of::<TdxRequest>());
}

#[cfg(any(target_os = "linux", test))]
use abi::*;

#[cfg(target_os = "linux")]
fn snp_report(bind: &[u8; 64]) -> Result<Vec<u8>> {
    use std::os::fd::AsRawFd;

    let device = std::fs::File::open(Platform::SevSnp.device())?;
    let request = SnpRequest {
        user_data: *bind,
        vmpl: 0,
        reserved: [0; 28],
    };
    let mut response = vec![0u8; SNP_RESPONSE];
    let mut message = SnpMessage {
        version: 1,
        request: &request as *const SnpRequest as u64,
        response: response.as_mut_ptr() as u64,
        error: 0,
    };
    // SAFETY: the driver reads `message`, and through it a request of the size
    // the ioctl number declares and a response buffer of the size the ABI
    // fixes. Both outlive the call.
    let rc = unsafe {
        libc::ioctl(
            device.as_raw_fd(),
            SNP_GET_REPORT as libc::Ioctl,
            &mut message as *mut SnpMessage,
        )
    };
    if rc != 0 {
        bail!(
            "SNP_GET_REPORT: {} (firmware error {:#x})",
            std::io::Error::last_os_error(),
            message.error
        );
    }
    let end = SNP_REPORT_OFFSET + SNP_REPORT_LENGTH;
    Ok(response[SNP_REPORT_OFFSET..end].to_vec())
}

#[cfg(target_os = "linux")]
fn tdx_report(bind: &[u8; 64]) -> Result<Vec<u8>> {
    use std::os::fd::AsRawFd;

    let device = std::fs::File::open(Platform::Tdx.device())?;
    let mut request = TdxRequest {
        report_data: *bind,
        report: [0; TDX_REPORT_LENGTH],
    };
    // SAFETY: as above — one structure, the size the ioctl number declares.
    let rc = unsafe {
        libc::ioctl(
            device.as_raw_fd(),
            TDX_GET_REPORT as libc::Ioctl,
            &mut request as *mut TdxRequest,
        )
    };
    if rc != 0 {
        bail!("TDX_CMD_GET_REPORT0: {}", std::io::Error::last_os_error());
    }
    Ok(request.report.to_vec())
}

#[cfg(not(target_os = "linux"))]
fn snp_report(_bind: &[u8; 64]) -> Result<Vec<u8>> {
    bail!("SEV-SNP reports come from a Linux guest device")
}

#[cfg(not(target_os = "linux"))]
fn tdx_report(_bind: &[u8; 64]) -> Result<Vec<u8>> {
    bail!("TDX reports come from a Linux guest device")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request structures are the sizes the kernel headers define. The
    /// ioctl number encodes that size, so a structure that drifts would send a
    /// number no driver answers — this is the check the missing hardware
    /// cannot perform for us.
    #[test]
    fn the_request_structures_match_the_kernel_abi() {
        assert_eq!(std::mem::size_of::<SnpRequest>(), 96);
        assert_eq!(std::mem::size_of::<SnpMessage>(), 32);
        assert_eq!(std::mem::size_of::<TdxRequest>(), 1088);
        assert_eq!(std::mem::size_of::<TdxRequest>(), 64 + TDX_REPORT_LENGTH);
        // The report we slice out has to lie inside the buffer the driver fills.
        let end = SNP_REPORT_OFFSET + SNP_REPORT_LENGTH;
        assert_eq!(
            end.min(SNP_RESPONSE),
            end,
            "the report overruns the response"
        );
    }

    /// The encoding, against the numbers `_IOWR` produces for these three
    /// arguments (they appear as literals in driver documentation and in every
    /// strace of a guest attestation).
    #[test]
    fn the_ioctl_numbers_are_the_documented_ones() {
        assert_eq!(SNP_GET_REPORT, 0xC020_5300);
        assert_eq!(TDX_GET_REPORT, 0xC440_5401);
    }

    /// Detection is device presence, and on a machine without one the status
    /// says so rather than inventing a platform.
    #[test]
    fn a_machine_without_the_device_reports_none() {
        if platform().is_none() {
            assert_eq!(status(&[0u8; 64]), Status::default());
            assert_eq!(Status::default().platform, "none");
            assert!(Status::default().report.is_none());
        }
    }

    #[test]
    fn a_platform_names_its_device() {
        assert_eq!(Platform::SevSnp.device(), "/dev/sev-guest");
        assert_eq!(Platform::Tdx.device(), "/dev/tdx_guest");
        assert_eq!(Platform::SevSnp.label(), "sev-snp");
        assert_eq!(Platform::Tdx.label(), "tdx");
    }
}
