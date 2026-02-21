# shuru

Local-first microVM sandbox for AI agents on macOS.

Shuru boots lightweight Linux VMs using Apple's Virtualization.framework. Each sandbox is ephemeral -- the rootfs resets on every run -- giving agents a disposable environment to execute code, install packages, and run tools without touching your host.

## Requirements

- macOS on Apple Silicon

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/superhq-ai/shuru/main/install.sh | sh
```

This installs the `shuru` binary to `~/.local/bin/`.

## Usage

```sh
# Interactive shell (default when no command given)
shuru run

# Run a command in a sandbox
shuru run -- echo hello

# Interactive shell with network access
shuru run --allow-net

# Custom resources
shuru run --cpus 4 --memory 4096 -- make -j4

# Use a project config file
shuru run --config myproject.json
```

## Commands

| Command | Description |
|---------|-------------|
| `shuru run [OPTIONS] [-- <cmd>]` | Boot a VM and run a command (defaults to `/bin/sh`) |
| `shuru init [--force]` | Download or update OS image assets |
| `shuru upgrade` | Upgrade shuru to the latest release |

### `shuru run` flags

| Flag | Description |
|------|-------------|
| `--allow-net` | Enable network access (NAT). Disabled by default. |
| `--cpus N` | Number of CPU cores (default: 2) |
| `--memory MB` | Memory in megabytes (default: 2048) |
| `--config PATH` | Path to config file (default: `./shuru.json`) |
| `--console` | Attach to raw serial console instead of running a command (for debugging) |
| `--kernel PATH` | Path to kernel image (env: `SHURU_KERNEL`) |
| `--rootfs PATH` | Path to rootfs ext4 image (env: `SHURU_ROOTFS`) |
| `--initrd PATH` | Path to initramfs (env: `SHURU_INITRD`) |

## Config file

Shuru looks for a `shuru.json` in the current directory (or at the path given by `--config`). All fields are optional:

```json
{
  "cpus": 4,
  "memory": 4096,
  "allow_net": true,
  "command": ["python", "script.py"]
}
```

Resolution order per field: **CLI flag > config file > default**.

## Architecture

```
┌─────────────────────────────────────────────┐
│  macOS Host                                 │
│                                             │
│  shuru-cli ──► shuru-vm ──► shuru-darwin    │
│                   │      (Virtualization.framework)
│                   │                         │
│               ┌───┴────────────┐            │
│               │  vsock :1024   │            │
│               └───┬────────────┘            │
├───────────────────┼─────────────────────────┤
│  Linux Guest      │                         │
│               ┌───┴────────────┐            │
│               │  shuru-guest   │            │
│               │  (PID 1 init)  │            │
│               └────────────────┘            │
│  Alpine Linux 3.21 / linux-virt 6.12        │
└─────────────────────────────────────────────┘
```

Shuru is a Rust workspace with four crates:

| Crate | Role |
|-------|------|
| **shuru-cli** | CLI binary (`shuru`). Parses flags/config, manages assets, drives the VM lifecycle. |
| **shuru-vm** | High-level sandbox API. Builds VM configuration, exposes `exec()` (piped) and `shell()` (interactive PTY) over vsock. |
| **shuru-darwin** | Low-level Objective-C bindings to Apple's Virtualization.framework via `objc2`. |
| **shuru-guest** | PID 1 init for the guest VM. Cross-compiled to `aarch64-unknown-linux-musl`. Handles vsock protocol, process spawning, PTY allocation, and networking. |

### Boot sequence

1. **Kernel** -- Alpine `linux-virt` (ARM64, uncompressed `Image`). VirtIO core drivers are built-in.
2. **Initramfs** -- Loads kernel modules that are not built-in (`virtio_blk`, `ext4`, `af_packet`, `virtio_net`, `vsock`, etc.), waits for `/dev/vda`, mounts the rootfs, runs DHCP on `eth0` if a network device is present, then `switch_root` into the rootfs.
3. **Rootfs** -- A 512 MB ext4 image containing Alpine minirootfs. `shuru-guest` is installed at `/usr/bin/shuru-init` and runs as PID 1.
4. **Guest init** -- Mounts filesystems (`proc`, `sysfs`, `devtmpfs`, `devpts`, `tmpfs`), sets hostname to `shuru`, detects if networking was already configured by the initramfs (skips DHCP if so), then listens for vsock connections on port 1024.

### Sandbox permissions

Sandboxes are locked down by default. Permissions are opt-in:

| Permission | Flag | Default | Effect |
|------------|------|---------|--------|
| Network | `--allow-net` | off | Attaches a VirtIO NIC with NAT to the VM |

Without `--allow-net`, no network device is attached to the VM -- the guest has no `eth0` and cannot make any outbound connections.

### Networking

When `--allow-net` is set, Shuru attaches a VirtIO network device with a NAT attachment (via `VZNATNetworkDeviceAttachment`). macOS handles all routing transparently.

DHCP runs in two stages to avoid timing races with the host NAT attachment:

1. **Initramfs (primary)** -- After loading the `af_packet` and `virtio_net` modules, the initramfs runs `udhcpc` (busybox) before `switch_root`. This runs early enough that the host NAT is ready, and configures the IP, gateway, and writes DNS to the rootfs `/etc/resolv.conf`.
2. **Guest init (fallback)** -- `shuru-guest` checks if `eth0` already has an IP via `SIOCGIFADDR`. If it does, DHCP is skipped entirely. If not (e.g. custom initramfs without DHCP), it falls back to a built-in DHCP client.

If `eth0` doesn't exist (no `--allow-net`), network setup is silently skipped in both stages.

### Host-guest communication (vsock)

All command execution goes over a VirtIO socket (vsock) on port 1024, using a JSON-lines protocol:

**Host -> Guest** (exec request):
```json
{"argv": ["/bin/sh", "-c", "echo hello"], "env": {}, "tty": true, "rows": 24, "cols": 80}
```

**Guest -> Host** (streamed responses):
```json
{"type": "stdout", "data": "hello\n"}
{"type": "exit", "code": 0}
```

**Interactive mode** -- when stdin is a TTY, the host puts the terminal in raw mode and relays keystrokes as `{"type": "stdin", "data": "..."}` messages. The guest allocates a PTY (`openpty`) and handles window resize (`{"type": "resize", "rows": 24, "cols": 80}`) via `TIOCSWINSZ`.

## Development

```sh
# Build the guest init (needs aarch64-linux-musl-gcc cross-compiler)
cargo build -p shuru-guest --target aarch64-unknown-linux-musl --release

# Prepare rootfs, kernel, and initramfs (needs Docker)
./scripts/prepare-rootfs.sh

# Build the CLI
cargo build -p shuru-cli

# Codesign (required for Virtualization.framework entitlement)
codesign --entitlements shuru.entitlements --force -s - target/debug/shuru

# Run
./target/debug/shuru run -- echo hello
```

## Bugs

File issues at [github.com/superhq-ai/shuru/issues](https://github.com/superhq-ai/shuru/issues).
