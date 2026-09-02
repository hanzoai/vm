# Changelog

## 2.0.0

`cargo install hanzo-vm` now installs this CLI. The `hanzo-vm` name on
crates.io previously carried an unrelated EVM (versions up to 1.1.22, since
renamed to `hanzo-evm`); 2.0.0 takes the name over for the microVM and jumps
past those versions. No code change from 0.1.3 beyond the version.

- The library package `vm` is published as `vm-core` (the `vm` name on
  crates.io belongs to someone else); its library target is still named
  `vm`, so `use vm::…` is unchanged.
- Every crate in the workspace and the TypeScript SDK move to 2.0.0
  together; the OS image tag the CLI downloads follows the crate version
  as before.

## 0.1.3

Boot-time release: `run -- true` medians drop from 0.41/0.41/0.71 s to
0.42/0.27/0.26 s (macOS VZ / Linux x86_64 cloud-hypervisor / Linux aarch64
KVM; the macOS median was 0.58 s re-measured on the same host before these
changes).

- Throwaway work disks clone to tmpfs on non-reflink filesystems (Linux):
  no writeback, page-free teardown, multi-second stall variance gone.
  Sparse clone falls back to pread/pwrite across filesystems (kernels
  >= 5.19 refuse cross-fs `copy_file_range`).
- Quiet exec runs boot without a serial console device or `console=` —
  the guest skips the device probe and console registration; dmesg still
  captures everything. `--verbose` now names the backend's real console,
  which makes kernel output visible on the KVM backend for the first time
  (PL011 `ttyAMA0`, not `hvc0`).
- KVM device threads (vsock irq, net rx) wake on an eventfd at reset
  instead of sleeping out a poll timeout — teardown loses its 0.5 s tail.
- cloud-hypervisor sockets move to a private per-pid dir cleaned on drop;
  they no longer collide when work disks share a tmpfs directory.
- x86_64 kernel: `CONFIG_X86_X2APIC` — cloud-hypervisor describes vCPUs as
  x2apic MADT entries, so guests booted with one CPU no matter how many
  were requested. Plus `CONFIG_DEFERRED_STRUCT_PAGE_INIT` (measured win on
  x86; a wash on arm64, left off there).
- x86_64 rootfs: ships the current vm-guest — the 0.1.2 image carried an
  older build that never configured eth0, so `--allow-net` had no network.

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
