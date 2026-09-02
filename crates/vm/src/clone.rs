//! Linux rootfs clone. The base image is a 4 GB sparse ext4 file; a plain
//! `fs::copy` materializes every hole as written zeros (~1.2 s on ext4), so
//! clone with the cheapest mechanism the filesystem offers:
//!
//! 1. `ioctl(FICLONE)` — free reflink on XFS/btrfs.
//! 2. Sparse copy — walk `lseek(SEEK_DATA)`/`lseek(SEEK_HOLE)` extents,
//!    `copy_file_range` only the data, `ftruncate` to full size so holes
//!    stay holes.
//! 3. `fs::copy` — filesystems without SEEK_HOLE.
#![allow(unsafe_code)]

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

use anyhow::{Context, Result};

/// Clone `src` to `dst`, preserving sparseness where the filesystem allows.
pub fn clone_file(src: &str, dst: &str) -> Result<()> {
    let sf = File::open(src).with_context(|| format!("failed to open {}", src))?;
    let df = File::create(dst).with_context(|| format!("failed to create {}", dst))?;
    if reflink(&sf, &df).is_ok() {
        return Ok(());
    }
    if sparse_copy(&sf, &df).is_ok() {
        return Ok(());
    }
    drop(df); // fs::copy reopens and truncates any partial sparse copy
    std::fs::copy(src, dst).with_context(|| format!("failed to copy {} -> {}", src, dst))?;
    Ok(())
}

/// Reflink `src` to `dst` (FICLONE), no fallback. Fails on filesystems
/// without reflink (ext4, tmpfs) and across filesystems (EXDEV); the
/// caller decides where the working copy should live in that case.
pub fn reflink_file(src: &str, dst: &str) -> io::Result<()> {
    let sf = File::open(src)?;
    let df = File::create(dst)?;
    let r = reflink(&sf, &df);
    if r.is_err() {
        drop(df);
        let _ = std::fs::remove_file(dst);
    }
    r
}

fn reflink(src: &File, dst: &File) -> io::Result<()> {
    // EOPNOTSUPP on ext4/tmpfs; the caller falls through to sparse_copy.
    let ret = unsafe { libc::ioctl(dst.as_raw_fd(), libc::FICLONE, src.as_raw_fd()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn sparse_copy(src: &File, dst: &File) -> io::Result<()> {
    let len = src.metadata()?.len();
    dst.set_len(len)?;
    let (sfd, dfd) = (src.as_raw_fd(), dst.as_raw_fd());
    // copy_file_range refuses cross-filesystem copies (EXDEV, kernels
    // >= 5.19); pread/pwrite covers that at page-cache speed.
    let mut use_rw = false;
    let mut buf = Vec::new();
    let mut off: i64 = 0;
    while (off as u64) < len {
        let data = unsafe { libc::lseek(sfd, off, libc::SEEK_DATA) };
        if data < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ENXIO) {
                break; // only holes from `off` to EOF
            }
            return Err(err); // EINVAL: no SEEK_DATA on this fs
        }
        let end = unsafe { libc::lseek(sfd, data, libc::SEEK_HOLE) };
        if end < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut pos = data;
        while pos < end {
            if use_rw {
                if buf.is_empty() {
                    buf = vec![0u8; 1 << 20];
                }
                let want = ((end - pos) as usize).min(buf.len());
                let n =
                    unsafe { libc::pread(sfd, buf.as_mut_ptr() as *mut libc::c_void, want, pos) };
                if n <= 0 {
                    return Err(io::Error::last_os_error());
                }
                let n = n as i64;
                let mut written: i64 = 0;
                while written < n {
                    let w = unsafe {
                        libc::pwrite(
                            dfd,
                            buf.as_ptr().add(written as usize) as *const libc::c_void,
                            (n - written) as usize,
                            pos + written,
                        )
                    };
                    if w <= 0 {
                        return Err(io::Error::last_os_error());
                    }
                    written += w as i64;
                }
                pos += n;
                continue;
            }
            let (mut s_off, mut d_off) = (pos, pos);
            let n = unsafe {
                libc::copy_file_range(sfd, &mut s_off, dfd, &mut d_off, (end - pos) as usize, 0)
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                match err.raw_os_error() {
                    Some(libc::EXDEV) | Some(libc::EINVAL) | Some(libc::ENOSYS) => {
                        use_rw = true;
                        continue;
                    }
                    _ => return Err(err),
                }
            }
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "copy_file_range returned 0 before extent end",
                ));
            }
            pos += n as i64;
        }
        off = end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::MetadataExt;

    const HOLE: u64 = 100 * 1024 * 1024;

    #[test]
    fn clone_preserves_content_and_holes() {
        let dir = std::env::temp_dir().join(format!("vm-clone-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.img");
        let dst = dir.join("dst.img");

        let mut f = File::create(&src).unwrap();
        f.write_all(b"head data").unwrap();
        f.seek(SeekFrom::Start(HOLE)).unwrap();
        f.write_all(b"tail data").unwrap();
        f.set_len(HOLE + 4096).unwrap();
        drop(f);

        // On ext4/tmpfs the FICLONE attempt fails (EOPNOTSUPP) and the
        // sparse copy takes over; on XFS/btrfs the reflink succeeds. Either
        // way the result must be identical and stay sparse.
        clone_file(src.to_str().unwrap(), dst.to_str().unwrap()).unwrap();

        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dst).unwrap());
        let meta = std::fs::metadata(&dst).unwrap();
        assert_eq!(meta.len(), HOLE + 4096);
        let allocated = meta.blocks() * 512;
        assert!(
            allocated < 1024 * 1024,
            "destination materialized holes: {} bytes allocated for {} apparent",
            allocated,
            meta.len()
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clone_across_filesystems_stays_sparse() {
        // ext4 -> tmpfs: copy_file_range returns EXDEV on kernels >= 5.19,
        // so this exercises the pread/pwrite fallback.
        if !std::path::Path::new("/dev/shm").is_dir() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vm-clone-x-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.img");
        let dst = format!("/dev/shm/vm-clone-x-{}.img", std::process::id());

        let mut f = File::create(&src).unwrap();
        f.write_all(b"head data").unwrap();
        f.seek(SeekFrom::Start(HOLE)).unwrap();
        f.write_all(b"tail data").unwrap();
        f.set_len(HOLE + 4096).unwrap();
        drop(f);

        clone_file(src.to_str().unwrap(), &dst).unwrap();

        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&dst).unwrap());
        let meta = std::fs::metadata(&dst).unwrap();
        assert_eq!(meta.len(), HOLE + 4096);
        let allocated = meta.blocks() * 512;
        assert!(
            allocated < 1024 * 1024,
            "destination materialized holes: {} bytes allocated",
            allocated
        );

        std::fs::remove_file(&dst).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
