# shuru

Local-first microVM sandbox for AI agents on macOS.

Shuru boots lightweight Linux VMs using Apple's Virtualization.framework. Each sandbox is ephemeral – the rootfs resets on every run – giving agents a disposable environment to execute code, install packages, and run tools without touching your host.

## Requirements

- macOS on Apple Silicon

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/superhq-ai/shuru/main/install.sh | sh
```

This installs the `shuru` binary to `~/.local/bin/`.

## Usage

```sh
# Run a command in a sandbox
shuru run -- echo hello

# Interactive shell with network access
shuru run --net -- sh

# Custom resources
shuru run --cpus 4 --memory 4096 -- make -j4
```

## Commands

| Command | Description |
|---------|-------------|
| `shuru run [--net] [--cpus N] [--memory MB] -- <cmd>` | Boot a VM and run a command |
| `shuru init [--force]` | Download or update OS image assets |
| `shuru upgrade` | Upgrade shuru to the latest release |

## Bugs

File issues at [github.com/superhq-ai/shuru/issues](https://github.com/superhq-ai/shuru/issues).
