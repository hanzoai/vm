guest_target := "aarch64-unknown-linux-musl"
binary := "target/debug/hanzo-vm"
data_dir := env_var_or_default("HANZO_VM_HOME", env_var("HOME") + "/.hanzo/vm")

# List available recipes
default:
    @just --list

# Build the guest init binary (cross-compiled to aarch64 musl)
build-guest:
    cargo build -p vm-guest --target {{ guest_target }} --release

# Build the CLI binary (debug)
build-cli:
    cargo build -p hanzo-vm

# Codesign the CLI binary with the virtualization entitlement
codesign:
    codesign --entitlements vm.entitlements --force -s - {{ binary }}

# Build everything: guest + CLI + codesign
build: build-guest build-cli codesign

# Build the kernel, initramfs and rootfs into the data dir
prepare-rootfs:
    ./scripts/prepare-rootfs.sh

# Run a command inside the VM
run *args:
    {{ binary }} run -- {{ args }}

# Open an interactive shell in the VM
shell:
    {{ binary }} run -- sh

# Full setup from scratch: rootfs + build
setup: prepare-rootfs build

# Check all crates compile (host targets only)
check:
    cargo check --workspace

# Clippy with CI's settings
clippy:
    cargo clippy --all-targets -- -D warnings

# Install the release binary to ~/.local/bin/hanzo-vm
install: build-guest
    cargo build -p hanzo-vm --release
    codesign --entitlements vm.entitlements --force -s - target/release/hanzo-vm
    mkdir -p ~/.local/bin
    cp target/release/hanzo-vm ~/.local/bin/hanzo-vm
    mkdir -p {{ data_dir }}
    cargo pkgid -p hanzo-vm | sed 's/.*#//' > {{ data_dir }}/VERSION

# Tag and push a release (runs .hanzo/workflows/release.yml)
release version:
    git tag -a "v{{ version }}" -m "Release v{{ version }}"
    git push origin "v{{ version }}"
