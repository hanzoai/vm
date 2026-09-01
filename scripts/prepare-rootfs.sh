#!/bin/bash
# Builds the guest image into the data dir: Image (kernel), initramfs.cpio.gz,
# rootfs.ext4 (Debian, with the guest binary at /usr/bin/vm-guest).
#
# Runs natively on an arm64 Linux host. Needs: debootstrap, e2fsprogs
# (mke2fs -d), busybox-static, pax-utils (lddtree), cpio, and for the kernel
# build-essential bc flex bison libelf-dev libssl-dev curl xz-utils. debootstrap
# needs root; sudo is used when not already root. On macOS the script re-runs
# itself inside a Debian arm64 container, so only Docker is needed there.
set -euo pipefail

DEBIAN_RELEASE="trixie"
DATA_DIR="${HANZO_VM_HOME:-$HOME/.hanzo/vm}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"
ROOTFS_SIZE_MB=1024

# The guest image is built for the host architecture (macOS builds arm64
# in a container below).
case "$(uname -m)" in
    x86_64) GUEST_TARGET="x86_64-unknown-linux-musl"; DEB_ARCH="amd64" ;;
    *)      GUEST_TARGET="aarch64-unknown-linux-musl"; DEB_ARCH="arm64" ;;
esac
GUEST_BINARY="${REPO_DIR}/target/${GUEST_TARGET}/release/vm-guest"

if [ ! -f "$GUEST_BINARY" ]; then
    echo "ERROR: guest binary not found at ${GUEST_BINARY}"
    echo "       Run: cargo build -p vm-guest --target ${GUEST_TARGET} --release"
    exit 1
fi

mkdir -p "$DATA_DIR"

if [ "$(uname)" = "Darwin" ]; then
    command -v docker >/dev/null || { echo "ERROR: Docker is required on macOS"; exit 1; }
    exec docker run --rm --platform linux/arm64/v8 \
        -v "${REPO_DIR}:/src:ro" -v "${DATA_DIR}:/out" -e HANZO_VM_HOME=/out \
        "debian:${DEBIAN_RELEASE}-slim" /src/scripts/prepare-rootfs.sh
fi

SUDO=""
[ "$(id -u)" = 0 ] || SUDO=sudo

for tool in debootstrap mke2fs busybox lddtree cpio gcc bc flex bison curl xz; do
    if ! command -v "$tool" >/dev/null; then
        $SUDO apt-get update -qq
        $SUDO apt-get install -y -qq debootstrap e2fsprogs busybox-static pax-utils cpio \
            build-essential bc flex bison libelf-dev libssl-dev curl xz-utils ca-certificates
        break
    fi
done

if [ -f "${DATA_DIR}/Image" ]; then
    echo "==> Kernel already present."
else
    "${SCRIPT_DIR}/build-kernel.sh"
fi

WORK="$(mktemp -d)"
trap '$SUDO rm -rf "$WORK"' EXIT

if [ -f "${DATA_DIR}/initramfs.cpio.gz" ]; then
    echo "==> Initramfs already present."
else
    echo "==> Building initramfs..."
    IR="${WORK}/initramfs"
    mkdir -p "$IR"/bin "$IR"/etc "$IR"/proc "$IR"/dev "$IR"/newroot
    cp "$(command -v busybox)" "$IR/bin/busybox"
    for cmd in sh mount umount switch_root cp chmod echo ifconfig route cat; do
        ln -sf busybox "$IR/bin/$cmd"
    done
    lddtree -l /usr/sbin/e2fsck /usr/sbin/resize2fs | sort -u | cpio --quiet -pmdL "$IR"
    install -m 755 "$GUEST_BINARY" "$IR/bin/vm-guest"
    cat > "$IR/init" <<'INIT'
#!/bin/sh
mount -t proc none /proc
mount -t devtmpfs none /dev
/usr/sbin/e2fsck -p /dev/vda > /dev/null 2>&1 || true
/usr/sbin/resize2fs /dev/vda > /dev/null 2>&1 || true
mount -t ext4 /dev/vda /newroot
cp /bin/vm-guest /newroot/usr/bin/vm-guest
chmod 755 /newroot/usr/bin/vm-guest
if ifconfig eth0 up 2>/dev/null; then
    ifconfig eth0 10.0.0.2 netmask 255.255.255.0 up
    route add default gw 10.0.0.1
    echo "nameserver 10.0.0.1" > /newroot/etc/resolv.conf
fi
umount /proc
exec switch_root /newroot /usr/bin/vm-guest
INIT
    chmod 755 "$IR/init"
    (cd "$IR" && find . | cpio -o -H newc 2>/dev/null | gzip > "${DATA_DIR}/initramfs.cpio.gz")
    echo "    Initramfs created: $(du -h "${DATA_DIR}/initramfs.cpio.gz" | cut -f1)"
fi

if [ -f "${DATA_DIR}/rootfs.ext4" ]; then
    echo "==> Rootfs already present."
else
    echo "==> Building Debian ${DEBIAN_RELEASE} rootfs (${ROOTFS_SIZE_MB}MB)..."
    ROOT="${WORK}/rootfs"
    $SUDO debootstrap --arch="$DEB_ARCH" --variant=minbase "$DEBIAN_RELEASE" "$ROOT" http://deb.debian.org/debian

    $SUDO mkdir -p "${ROOT}/etc/dpkg/dpkg.cfg.d"
    cat <<'DPKG' | $SUDO tee "${ROOT}/etc/dpkg/dpkg.cfg.d/01-nodoc" > /dev/null
path-exclude /usr/share/doc/*
path-exclude /usr/share/man/*
path-exclude /usr/share/info/*
path-exclude /usr/share/locale/*
path-include /usr/share/locale/en*
DPKG

    $SUDO chroot "$ROOT" apt-get update -qq
    $SUDO chroot "$ROOT" apt-get install -y -qq --no-install-recommends \
        ca-certificates curl git iproute2 iptables nftables \
        openssh-client jq less procps xz-utils libgomp1 libatomic1 > /dev/null 2>&1
    $SUDO rm -rf "${ROOT}/usr/share/doc/"* "${ROOT}/usr/share/man/"* "${ROOT}/usr/share/info/"*
    $SUDO find "${ROOT}/usr/share/locale" -mindepth 1 -maxdepth 1 ! -name "en*" -exec rm -rf {} + 2>/dev/null || true
    $SUDO chroot "$ROOT" apt-get clean
    $SUDO rm -rf "${ROOT}/var/lib/apt/lists/"*

    $SUDO install -m 755 "$GUEST_BINARY" "${ROOT}/usr/bin/vm-guest"
    $SUDO mkdir -p "${ROOT}/proc" "${ROOT}/sys" "${ROOT}/dev" "${ROOT}/tmp" "${ROOT}/run"
    echo "hanzo-vm" | $SUDO tee "${ROOT}/etc/hostname" > /dev/null
    printf '127.0.0.1\tlocalhost\n127.0.1.1\thanzo-vm\n::1\tlocalhost ip6-localhost ip6-loopback\n' | $SUDO tee "${ROOT}/etc/hosts" > /dev/null
    echo "nameserver 8.8.8.8" | $SUDO tee "${ROOT}/etc/resolv.conf" > /dev/null

    truncate -s "${ROOTFS_SIZE_MB}M" "${DATA_DIR}/rootfs.ext4"
    $SUDO mke2fs -q -F -t ext4 -E lazy_itable_init=0 -d "$ROOT" "${DATA_DIR}/rootfs.ext4"
    echo "    Rootfs created: $(du -h "${DATA_DIR}/rootfs.ext4" | cut -f1)"
fi

echo ""
echo "==> Done!"
echo "    Kernel:     ${DATA_DIR}/Image"
echo "    Initramfs:  ${DATA_DIR}/initramfs.cpio.gz"
echo "    Rootfs:     ${DATA_DIR}/rootfs.ext4"
