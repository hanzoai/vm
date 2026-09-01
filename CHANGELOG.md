# Changelog

## 0.1.2

- Linux x86_64 backend (`crates/vm-ch`): drives cloud-hypervisor over its API
  socket, virtiofsd for mounts, an in-process vhost-user-net backend for
  `--allow-net`, and hybrid vsock. Guest assets build for the host arch
  (PVH vmlinux kernel, amd64 rootfs); OS tarballs are per-arch
  (`hanzo-vm-os-<tag>-<arch>.tar.gz`), so x86_64 hosts can cold-start from
  the release.
- Sparse-aware rootfs clone on Linux: `ioctl(FICLONE)` reflink where the
  filesystem supports it, else a SEEK_DATA/SEEK_HOLE + `copy_file_range`
  sparse copy — a plain copy of the 4 GB image materializes the holes.

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
