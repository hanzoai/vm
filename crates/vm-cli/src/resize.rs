use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use vm::default_data_dir;

use crate::measure;

/// Parse a human-readable size string (or plain MiB integer) into MiB (`u64`).
///
/// Supported forms:
/// - Plain integer (`"4096"` → `4096` MiB)
/// - MiB / MB / M (`"8192MiB"`, `"8192MB"`, `"8192M"`, `"512MiB"`)
/// - GiB / GB / G (`"32GiB"` → `32768`, `"8GiB"` → `8192`, `"32G"` → `32768`)
/// - TiB / TB / T (`"1TiB"` → `1048576`)
pub(crate) fn parse_size_mb(raw: &str) -> std::result::Result<u64, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("size cannot be empty".into());
    }
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '_')
        .unwrap_or(s.len());
    let num_part = s[..split].replace('_', "");
    let unit_part = s[split..].trim().to_ascii_lowercase();
    if num_part.is_empty() {
        return Err(format!("invalid size `{raw}`: missing numeric value"));
    }

    let mb = if unit_part.is_empty() || matches!(unit_part.as_str(), "m" | "mb" | "mib") {
        parse_scaled_u64(&num_part, 1, raw)?
    } else if matches!(unit_part.as_str(), "g" | "gb" | "gib") {
        parse_scaled_u64(&num_part, 1024, raw)?
    } else if matches!(unit_part.as_str(), "t" | "tb" | "tib") {
        parse_scaled_u64(&num_part, 1024 * 1024, raw)?
    } else {
        return Err(format!(
            "unsupported size unit in `{raw}` (expected MiB, GiB, TiB, or plain MiB integer)"
        ));
    };

    if mb == 0 {
        return Err(format!("size `{raw}` must be at least 1 MiB"));
    }
    Ok(mb)
}

fn parse_scaled_u64(num_part: &str, factor: u64, raw: &str) -> std::result::Result<u64, String> {
    if !num_part.contains('.') {
        let n: u64 = num_part
            .parse()
            .map_err(|_| format!("invalid integer size `{raw}`"))?;
        return n
            .checked_mul(factor)
            .ok_or_else(|| format!("size `{raw}` overflows u64 MiB"));
    }
    let f: f64 = num_part
        .parse()
        .map_err(|_| format!("invalid numeric size `{raw}`"))?;
    if !f.is_finite() || f <= 0.0 {
        return Err(format!("invalid size `{raw}`"));
    }
    let scaled = (f * (factor as f64)).round();
    if scaled <= 0.0 || scaled > (u64::MAX as f64) {
        return Err(format!("size `{raw}` out of range"));
    }
    Ok(scaled as u64)
}

/// Offline resize of a raw `.ext4` disk image or named `.ext4` checkpoint.
///
/// Growing extends the sparse image file (`f.set_len`) and updates its cached
/// launch measurement digest; shrinking below the existing image length is
/// refused so filesystem extents are never truncated offline.
pub(crate) fn run(checkpoint: Option<&str>, image: Option<&Path>, disk_mb: u64) -> Result<()> {
    let target_path = resolve_target_path(checkpoint, image)?;
    let previous_mb = resize_ext4_file(&target_path, disk_mb)?;
    measure::warm(&target_path.to_string_lossy());
    if disk_mb == previous_mb {
        println!(
            "hanzo-vm: {} already {} MiB",
            target_path.display(),
            disk_mb
        );
    } else {
        println!(
            "hanzo-vm: resized {} from {} MiB to {} MiB",
            target_path.display(),
            previous_mb,
            disk_mb
        );
    }
    Ok(())
}

fn resolve_target_path(checkpoint: Option<&str>, image: Option<&Path>) -> Result<PathBuf> {
    if let (Some(_), Some(_)) = (checkpoint, image) {
        bail!("specify either --checkpoint or --image, not both");
    }
    let data_dir = default_data_dir();
    if let Some(p) = image {
        if !p.exists() {
            bail!("disk image not found at {}", p.display());
        }
        return Ok(p.to_path_buf());
    }
    if let Some(name) = checkpoint {
        vm::validate_checkpoint_name(name).map_err(|e| anyhow::anyhow!(e))?;
        let idx = PathBuf::from(format!("{data_dir}/checkpoints/{name}.idx"));
        let ext4 = PathBuf::from(format!("{data_dir}/checkpoints/{name}.ext4"));
        if ext4.exists() {
            return Ok(ext4);
        }
        if idx.exists() {
            bail!(
                "checkpoint '{name}' is stored as CAS chunks ({}); pass `--disk-size {name}` at `hanzo-vm run` time",
                idx.display()
            );
        }
        bail!("checkpoint '{name}' not found");
    }
    let base = PathBuf::from(format!("{data_dir}/rootfs.ext4"));
    if !base.exists() {
        bail!(
            "base rootfs not found at {} — run `hanzo-vm init` or pass `--checkpoint <NAME>` / `--image <PATH>`",
            base.display()
        );
    }
    Ok(base)
}

pub(crate) fn resize_ext4_file(path: &Path, disk_mb: u64) -> Result<u64> {
    let f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let current_bytes = f
        .metadata()
        .with_context(|| format!("stat {}", path.display()))?
        .len();
    let current_mb = current_bytes.div_ceil(1024 * 1024);
    let target_bytes = disk_mb
        .checked_mul(1024 * 1024)
        .ok_or_else(|| anyhow::anyhow!("requested disk size {disk_mb} MiB overflows byte size"))?;
    if target_bytes < current_bytes {
        bail!(
            "cannot shrink {} from {} MiB to {} MiB offline",
            path.display(),
            current_mb,
            disk_mb
        );
    }
    if target_bytes > current_bytes {
        f.set_len(target_bytes)
            .with_context(|| format!("extending {} to {disk_mb} MiB", path.display()))?;
    }
    Ok(current_mb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_readable_and_plain_mib_sizes() {
        assert_eq!(parse_size_mb("32GiB").unwrap(), 32768);
        assert_eq!(parse_size_mb("32GB").unwrap(), 32768);
        assert_eq!(parse_size_mb("32G").unwrap(), 32768);
        assert_eq!(parse_size_mb("8GiB").unwrap(), 8192);
        assert_eq!(parse_size_mb("8192MiB").unwrap(), 8192);
        assert_eq!(parse_size_mb("8192MB").unwrap(), 8192);
        assert_eq!(parse_size_mb("8192M").unwrap(), 8192);
        assert_eq!(parse_size_mb("4096").unwrap(), 4096);
        assert_eq!(parse_size_mb("512MiB").unwrap(), 512);
        assert_eq!(parse_size_mb("1TiB").unwrap(), 1_048_576);
        assert_eq!(parse_size_mb("1.5GiB").unwrap(), 1536);

        assert!(parse_size_mb("").is_err());
        assert!(parse_size_mb("0").is_err());
        assert!(parse_size_mb("0GiB").is_err());
        assert!(parse_size_mb("16KiB").is_err());
        assert!(parse_size_mb("-4GiB").is_err());
    }

    #[test]
    fn extends_disk_image_offline_and_refuses_shrink() {
        let dir = tempfile::tempdir().unwrap();
        let img = dir.path().join("disk.ext4");
        let f = std::fs::File::create(&img).unwrap();
        f.set_len(64 * 1024 * 1024).unwrap();
        drop(f);

        let prev = resize_ext4_file(&img, 128).unwrap();
        assert_eq!(prev, 64);
        assert_eq!(std::fs::metadata(&img).unwrap().len(), 128 * 1024 * 1024);

        let err = resize_ext4_file(&img, 32).unwrap_err().to_string();
        assert!(err.contains("cannot shrink"), "{err}");
    }
}
