# Changelog

## 0.1.0

First release of Hanzo VM.

- `hanzo-vm run` boots a microVM on Apple Virtualization.framework (macOS,
  Apple Silicon) or the experimental KVM backend (Linux arm64) and runs a
  command in it on a copy-on-write clone of the root image.
- VirtioFS mounts (read-only with a tmpfs overlay, or `:rw` with
  `--allow-host-writes`), vsock port forwarding, disk checkpoints, a host-side
  proxy with per-host network policy and secret substitution.
- Config in `vm.json`; data in `~/.hanzo/vm` (`HANZO_VM_HOME`).
- Rust SDK `vm-sdk`; TypeScript SDK `@hanzo/vm` over `hanzo-vm run --stdio`.
