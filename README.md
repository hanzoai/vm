# shuru

Local-first microVM sandbox for AI agents on macOS.

Shuru boots lightweight Linux VMs using Apple's Virtualization.framework. Each sandbox is ephemeral: the rootfs resets on every run, giving agents a disposable environment to execute code, install packages, and run tools without touching your host.

## Requirements

- macOS on Apple Silicon

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/superhq-ai/shuru/main/install.sh | sh
```

## Usage

```sh
# Interactive shell
shuru run

# Run a command
shuru run -- echo hello

# With network access
shuru run --allow-net

# Custom resources
shuru run --cpus 4 --memory 4096 --disk-size 8192 -- make -j4
```

### Checkpoints

Checkpoints save the disk state so you can reuse an environment across runs.

```sh
# Set up an environment and save it
shuru checkpoint create myenv -- sh -c 'apk add python3 gcc'

# Run from a checkpoint (ephemeral -- changes are discarded)
shuru run --from myenv -- python3 script.py

# Branch from an existing checkpoint
shuru checkpoint create myenv2 --from myenv -- sh -c 'pip install numpy'

# List and delete
shuru checkpoint list
shuru checkpoint delete myenv
```

### Config file

Shuru loads `shuru.json` from the current directory (or `--config PATH`). All fields are optional; CLI flags take precedence.

```json
{
  "cpus": 4,
  "memory": 4096,
  "disk_size": 8192,
  "allow_net": true,
  "command": ["python", "script.py"]
}
```

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

## Bugs

File issues at [github.com/superhq-ai/shuru/issues](https://github.com/superhq-ai/shuru/issues).
