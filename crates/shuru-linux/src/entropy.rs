/// Placeholder — the guest gets entropy from the kernel's virtio-rng
/// if present in the FDT. Kept for API compatibility with shuru-darwin.
pub struct VirtioEntropyDevice;

impl VirtioEntropyDevice {
    pub fn new() -> Self {
        VirtioEntropyDevice
    }
}

impl Default for VirtioEntropyDevice {
    fn default() -> Self {
        Self::new()
    }
}
