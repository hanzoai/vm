# Hanzo VM

Hanzo's microVM layer. `hanzo-vm` boots a small Linux guest on the host you are
on, runs a command in it, and throws the disk away when the command exits. The
guest gets a copy-on-write clone of the root image, so packages installed and
files written inside a run never touch the host or the next run.

## Vocabulary

- **vm** — a microVM booted on a host we control. This repository.
- **sandbox** — a hosted lease of a vm with policy (`/v1/sandbox` in Hanzo
  Cloud). Not a local noun; nothing here is called a sandbox.
- **machine** — a host (Visor).

## Backends

- **macOS** — Apple Virtualization.framework. Apple Silicon only, arm64 guests
  only, no Rosetta. macOS 14 or later.
- **Linux arm64** — KVM backend (`crates/vm-linux`), hosts with `/dev/kvm`.
- **Linux x86_64** — cloud-hypervisor backend (`crates/vm-ch`), hosts with
  `/dev/kvm` plus the `cloud-hypervisor` and `virtiofsd` binaries (see
  [Linux hosts](#linux-hosts)).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/hanzoai/vm/main/install.sh | sh
```

or build from crates.io:

```sh
cargo install hanzo-vm
```

The installer puts `hanzo-vm` in `~/.local/bin` (cargo uses `~/.cargo/bin`).
The first `hanzo-vm run` downloads the
guest image (kernel, initramfs, root filesystem) from the matching GitHub
release into `~/.hanzo/vm`; set `HANZO_VM_HOME` to put it elsewhere.

## Linux hosts

Both Linux backends need read/write access to `/dev/kvm` (membership in the
`kvm` group, or an ACL).

arm64 connects to the guest through the host's vhost-vsock device; grant
access once with:

```sh
sudo setfacl -m u:$USER:rw /dev/vhost-vsock
```

x86_64 drives release binaries of cloud-hypervisor (verified with v53.0) and
virtiofsd (verified with 1.10.0), looked up on `PATH` and in `~/.local/bin`:

```sh
curl -fsSLo ~/.local/bin/cloud-hypervisor \
  https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v53.0/cloud-hypervisor-static
chmod +x ~/.local/bin/cloud-hypervisor
sudo apt install virtiofsd   # Debian/Ubuntu; installs /usr/libexec/virtiofsd
install -m 755 /usr/libexec/virtiofsd ~/.local/bin/virtiofsd
```

## Usage

```sh
# Interactive shell
hanzo-vm run

# Run a command
hanzo-vm run -- echo hello

# With network access
hanzo-vm run --allow-net

# Restrict to specific hosts
hanzo-vm run --allow-net --allow-host api.openai.com --allow-host registry.npmjs.org

# Use a specific DNS resolver
hanzo-vm run --allow-net --dns-resolver 1.1.1.1 -- curl https://example.com

# Custom resources
hanzo-vm run --cpus 4 --memory 4096 --disk-size 8192 -- make -j4
```

### Directory mounts

Host directories are shared into the guest over VirtioFS. A mount is read-only
by default; guest writes land in a tmpfs overlay that disappears with the vm.
Append `:rw` to write through to the host, which also needs
`--allow-host-writes`. Only paths under the current directory can be mounted.

```sh
# Read-only: the write stays in the overlay
hanzo-vm run --mount ./src:/workspace -- touch /workspace/test.txt
ls ./src/test.txt   # not found

# Read-write: the write reaches the host
hanzo-vm run --allow-host-writes --mount ./src:/workspace:rw -- touch /workspace/test.txt
ls ./src/test.txt   # found

# Several mounts
hanzo-vm run --mount ./src:/workspace --mount ./data:/data -- sh
```

Mounts can also be listed in `vm.json` (see [Config file](#config-file)).

### Port forwarding

Host ports are forwarded to guest ports over vsock, so this works without
`--allow-net` and without a network device in the guest.

```sh
# Install python3 into a checkpoint, then serve from it
hanzo-vm checkpoint create py --allow-net -- apt-get install -y python3
hanzo-vm run --from py -p 8080:8000 -- python3 -m http.server 8000

# From the host, in another terminal
curl http://127.0.0.1:8080/

# Several ports
hanzo-vm run -p 8080:80 -p 8443:443 -- nginx
```

Port forwards can also be listed in `vm.json`.

### Checkpoints

A checkpoint saves the disk after a command so later runs start from it.

```sh
# Build an environment and save it
hanzo-vm checkpoint create myenv --allow-net -- sh -c 'apt-get install -y python3 gcc'

# Run from it; changes made during the run are discarded
hanzo-vm run --from myenv -- python3 script.py

# Branch from an existing checkpoint
hanzo-vm checkpoint create myenv2 --from myenv --allow-net -- sh -c 'pip install numpy'

# List and delete
hanzo-vm checkpoint list
hanzo-vm checkpoint delete myenv
```

### Secrets

A secret never enters the guest. The guest sees a random placeholder in the
named environment variable; the host-side proxy substitutes the real value only
on HTTPS requests to the listed hosts.

```sh
hanzo-vm run --allow-net --secret API_KEY=OPENAI_API_KEY@api.openai.com -- curl https://api.openai.com/v1/models

hanzo-vm run --allow-net \
  --secret API_KEY=OPENAI_API_KEY@api.openai.com \
  --secret GH_TOKEN=GITHUB_TOKEN@api.github.com \
  -- sh
```

Format: `NAME=ENV_VAR@host1,host2`. `NAME` is the variable the guest sees,
`ENV_VAR` is the host variable holding the real value, and the hosts are where
the proxy substitutes it. A secret in `vm.json` can instead name a `command`
that mints the value and is re-run as it expires; see
[docs/rfcs/0002-refreshable-secrets.md](docs/rfcs/0002-refreshable-secrets.md).

### Config file

`hanzo-vm` reads `vm.json` from the current directory, or the file given by
`--config`. Every field is optional and flags take precedence.

```json
{
  "cpus": 4,
  "memory": 4096,
  "disk_size": 8192,
  "allow_net": true,
  "ports": ["8080:80"],
  "mounts": ["./src:/workspace", "./data:/data"],
  "command": ["python", "script.py"],
  "secrets": {
    "API_KEY": {
      "from": "OPENAI_API_KEY",
      "hosts": ["api.openai.com"]
    }
  },
  "network": {
    "allow": ["api.openai.com", "registry.npmjs.org"]
  }
}
```

`network.allow` restricts which hosts the guest can reach; omit it to allow all.

## SDK

`crates/vm-sdk` is the async Rust API over the same vm. `packages/sdk` is the
TypeScript package `@hanzo/vm`, which drives the `hanzo-vm` binary in
`--stdio` mode; see [packages/sdk/README.md](packages/sdk/README.md).

## Building from source

```sh
just build       # guest (aarch64 musl) + CLI + ad-hoc codesign
just install     # release CLI to ~/.local/bin/hanzo-vm
```

The guest image is built by `scripts/prepare-rootfs.sh` (kernel via
`scripts/build-kernel.sh`); it runs natively on arm64 and x86_64 Linux hosts
(building the guest needs `musl-tools` for the musl target) and inside Docker
on macOS. CI publishes the image with every release.

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
