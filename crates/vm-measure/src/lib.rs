//! What a VM is, stated as a number.
//!
//! A measurement is an ordered log of the things a launch loaded — the kernel,
//! the command line it was given, the root image, the workload — folded into a
//! single extend-only register:
//!
//! ```text
//! e_i     = SHA-384( name_i ‖ 0x00 ‖ SHA-384(content_i) )
//! R_0     = 0…0                                   (48 zero bytes)
//! R_{i+1} = SHA-384( R_i ‖ e_i )
//! ```
//!
//! The fold is the extend rule Intel TDX applies to a runtime measurement
//! register (RTMR := SHA-384(RTMR ‖ 48 bytes)), so a register built here
//! reproduces an RTMR that started at zero and was extended with the same
//! events, in the same order. SHA-384 is not a taste: it is the digest both
//! TDX registers and the AMD SEV-SNP launch measurement use, so nothing has to
//! be re-hashed at the boundary to hardware.
//!
//! Two properties follow from the shape and are the whole point:
//!
//! An event's NAME is inside its own digest, so an event cannot be replayed
//! under a different name, and the chain commits to order — a prefix of a log
//! and a whole log are different registers.
//!
//! Only content is hashed. The PATH a file was read from is recorded for the
//! reader and never enters the digest, so the same kernel and the same root
//! image measure identically on every machine that has them. A measurement is
//! reproducible by anyone holding the assets; it is not a fingerprint of one
//! host's filesystem.
//!
//! What this is NOT, stated once: computed on the host, a measurement is a
//! statement about what was launched, not evidence against the host that
//! launched it. A host that can substitute the kernel can also substitute the
//! program that hashes it. It becomes evidence when the register is bound to a
//! report signed by hardware the host does not control — see [`attest`].

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest as _, Sha384, Sha512};

pub mod attest;

/// The digest every register, event and file digest is taken with.
pub const ALGORITHM: &str = "sha384";

/// SHA-384 output, in bytes.
pub const LENGTH: usize = 48;

/// A SHA-384 digest: a register value, an event digest, a file digest. Carried
/// as bytes, written as hex.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Digest([u8; LENGTH]);

impl Digest {
    pub fn of(bytes: &[u8]) -> Digest {
        Digest(Sha384::digest(bytes).into())
    }

    /// The register before anything is extended into it.
    pub fn zero() -> Digest {
        Digest([0u8; LENGTH])
    }

    pub fn bytes(&self) -> &[u8; LENGTH] {
        &self.0
    }

    pub fn hex(&self) -> String {
        hex(&self.0)
    }

    pub fn parse(s: &str) -> Result<Digest> {
        let bytes = unhex(s).with_context(|| format!("not a hex digest: {s}"))?;
        let bytes: [u8; LENGTH] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("a {ALGORITHM} digest is {LENGTH} bytes"))?;
        Ok(Digest(bytes))
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.hex())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Digest::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// One measured thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// What was loaded: `kernel`, `cmdline`, `root`, `image`, `manifest`.
    pub name: String,
    /// Where the content came from — a filesystem path, or the text itself
    /// when the content IS text. For the reader; never hashed.
    pub source: String,
    /// SHA-384 of the content.
    pub digest: Digest,
}

impl Event {
    /// The 48 bytes the register extends with. The name is inside, so two
    /// events cannot trade places or masquerade as one another.
    pub fn extension(&self) -> Digest {
        let mut h = Sha384::new();
        h.update(self.name.as_bytes());
        h.update([0u8]);
        h.update(self.digest.0);
        Digest(h.finalize().into())
    }
}

/// An ordered log and, by folding it, one register value. The register is
/// derived, never stored: there is no way for the two to disagree.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    events: Vec<Event>,
}

impl Log {
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn push(&mut self, event: Event) {
        self.events.push(event);
    }

    /// Measure bytes the caller holds, saying where they came from or are
    /// going. `source` is for the reader and never enters the digest.
    pub fn bytes(&mut self, name: &str, source: &str, content: &[u8]) {
        self.push(Event {
            name: name.to_string(),
            source: source.to_string(),
            digest: Digest::of(content),
        });
    }

    /// Measure text — a kernel command line, an image reference, the VM's
    /// shape. The text is both the content and the source.
    pub fn text(&mut self, name: &str, value: &str) {
        self.bytes(name, value, value.as_bytes());
    }

    /// Measure a file's bytes, through `cache` — kernels and root images are
    /// hundreds of megabytes and do not change between boots.
    pub fn file(&mut self, name: &str, path: &Path, cache: &mut Cache) -> Result<()> {
        let digest = cache.digest(path)?;
        self.push(Event {
            name: name.to_string(),
            source: path.display().to_string(),
            digest,
        });
        Ok(())
    }

    /// Fold the log: `R_{i+1} = SHA-384(R_i ‖ e_i)`, from zero.
    pub fn register(&self) -> Digest {
        self.events.iter().fold(Digest::zero(), |r, e| {
            let mut h = Sha384::new();
            h.update(r.0);
            h.update(e.extension().0);
            Digest(h.finalize().into())
        })
    }
}

/// The wire shape of a log: the register value AND the events that produce it.
/// Written for a reader who wants the number without folding; verified on the
/// way back in, so a log whose value was edited is refused rather than trusted.
#[derive(Serialize, Deserialize)]
struct LogWire {
    value: Digest,
    events: Vec<Event>,
}

impl Serialize for Log {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        LogWire {
            value: self.register(),
            events: self.events.clone(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Log {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = LogWire::deserialize(d)?;
        let log = Log {
            events: wire.events,
        };
        if log.register() != wire.value {
            return Err(serde::de::Error::custom(format!(
                "log value {} does not fold from its events",
                wire.value
            )));
        }
        Ok(log)
    }
}

/// A launch and what it was asked to run, kept apart.
///
/// `launch` is what the hypervisor was handed: kernel, command line, root
/// image, shape. `workload` is what the started system was told to run. The
/// split is not decoration — the two are extended by different programs at
/// different times (the VM launcher, then whatever drives it), and on TDX they
/// map onto separate runtime registers. Folding them into one chain would lose
/// which half a verifier is entitled to predict.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measurement {
    pub launch: Log,
    pub workload: Log,
    /// What the guest's platform had to say for itself, asked once the chain
    /// is complete. An observation carried alongside the chain, never folded
    /// into it — see [`attest::Status`].
    pub hardware: attest::Status,
}

impl Measurement {
    /// The 64 bytes a hardware report carries as REPORT_DATA:
    /// `SHA-512(launch ‖ workload)`. SHA-512 because the field is 64 bytes and
    /// a report that pads is a report where the padding is unaccounted for.
    pub fn bind(&self) -> [u8; 64] {
        let mut h = Sha512::new();
        h.update(self.launch.register().0);
        h.update(self.workload.register().0);
        h.finalize().into()
    }

    pub fn bind_hex(&self) -> String {
        hex(&self.bind())
    }

    /// A bind read back from the hex it travels in — the inverse of
    /// [`Measurement::bind_hex`], for the wire that carries one to whatever
    /// will ask a platform to sign over it.
    pub fn parse_bind(s: &str) -> Result<[u8; 64]> {
        unhex(s)
            .and_then(|b| <[u8; 64]>::try_from(b).ok())
            .ok_or_else(|| anyhow::anyhow!("a bind is 64 bytes of hex"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&MeasurementWire::from(self)).expect("a measurement serializes")
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(&MeasurementWire::from(self))
            .expect("a measurement serializes")
    }

    pub fn from_json(s: &str) -> Result<Measurement> {
        let wire: MeasurementWire = serde_json::from_str(s).context("reading a measurement")?;
        if wire.algorithm != ALGORITHM {
            bail!(
                "measurement algorithm {} is not {ALGORITHM}",
                wire.algorithm
            );
        }
        let m = Measurement {
            launch: wire.launch,
            workload: wire.workload,
            hardware: wire.hardware,
        };
        if m.bind_hex() != wire.bind {
            bail!("measurement bind does not follow from its registers");
        }
        Ok(m)
    }
}

#[derive(Serialize, Deserialize)]
struct MeasurementWire {
    algorithm: String,
    launch: Log,
    workload: Log,
    bind: String,
    #[serde(default)]
    hardware: attest::Status,
}

impl From<&Measurement> for MeasurementWire {
    fn from(m: &Measurement) -> MeasurementWire {
        MeasurementWire {
            algorithm: ALGORITHM.to_string(),
            launch: m.launch.clone(),
            workload: m.workload.clone(),
            bind: m.bind_hex(),
            hardware: m.hardware.clone(),
        }
    }
}

// ---- file digests -------------------------------------------------------------

/// Remembered file digests, keyed by the file's identity.
///
/// The root image is a gigabyte and the checkpoint four; hashing them on every
/// boot would cost more than the boot. A digest is kept against the identity
/// the filesystem reports — device, inode, length, modification time — and
/// recomputed the moment any of those move. The cache is a host-local
/// convenience and carries no authority: it lives on the same disk as the
/// image it describes, so an attacker who can rewrite one can rewrite the
/// other. It saves time; it proves nothing. [`Cache::none`] skips it.
pub struct Cache {
    path: Option<PathBuf>,
    entries: BTreeMap<String, Entry>,
    dirty: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    device: u64,
    inode: u64,
    length: u64,
    modified: i64,
    modified_nanos: i64,
    digest: Digest,
}

impl Cache {
    /// Open the cache at `path`. A missing or unreadable file is an empty
    /// cache, never an error: a measurement that cannot be looked up is
    /// computed.
    pub fn open(path: impl Into<PathBuf>) -> Cache {
        let path = path.into();
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Cache {
            path: Some(path),
            entries,
            dirty: false,
        }
    }

    /// A cache that remembers nothing: every digest is computed from the file.
    pub fn none() -> Cache {
        Cache {
            path: None,
            entries: BTreeMap::new(),
            dirty: false,
        }
    }

    pub fn digest(&mut self, path: &Path) -> Result<Digest> {
        let identity = identity(path)?;
        let key = path.display().to_string();
        if let Some(hit) = self.entries.get(&key) {
            if hit.device == identity.device
                && hit.inode == identity.inode
                && hit.length == identity.length
                && hit.modified == identity.modified
                && hit.modified_nanos == identity.modified_nanos
            {
                return Ok(hit.digest);
            }
        }
        let digest = digest_file(path)?;
        self.entries.insert(key, Entry { digest, ..identity });
        self.dirty = true;
        Ok(digest)
    }

    /// Persist, atomically, when there is anything new to persist. Best
    /// effort: a cache we could not write is a slower next boot, not a failed
    /// one.
    pub fn save(&mut self) {
        let (Some(path), true) = (self.path.as_ref(), self.dirty) else {
            return;
        };
        let Ok(json) = serde_json::to_string(&self.entries) else {
            return;
        };
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            self.dirty = false;
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// SHA-384 of a file's bytes, read a megabyte at a time.
pub fn digest_file(path: &Path) -> Result<Digest> {
    let file =
        std::fs::File::open(path).with_context(|| format!("measuring {}", path.display()))?;
    let mut reader = std::io::BufReader::with_capacity(1 << 20, file);
    let mut hasher = Sha384::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Digest(hasher.finalize().into()))
}

#[cfg(unix)]
fn identity(path: &Path) -> Result<Entry> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).with_context(|| format!("measuring {}", path.display()))?;
    Ok(Entry {
        device: meta.dev(),
        inode: meta.ino(),
        length: meta.len(),
        modified: meta.mtime(),
        modified_nanos: meta.mtime_nsec(),
        digest: Digest::zero(),
    })
}

// ---- hex ----------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-384 of the empty string, from FIPS 180-4 — the algorithm is the one
    /// it claims to be.
    #[test]
    fn the_digest_is_sha384() {
        assert_eq!(
            Digest::of(b"").hex(),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da\
             274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(Digest::of(b"").bytes().len(), LENGTH);
    }

    #[test]
    fn a_digest_round_trips_through_hex() {
        let d = Digest::of(b"kernel");
        assert_eq!(Digest::parse(&d.hex()).unwrap(), d);
        assert!(Digest::parse("not hex").is_err());
        assert!(Digest::parse("ab").is_err(), "wrong length is refused");
    }

    /// The fold is exactly SHA-384(R ‖ e), from zero — computed here by hand
    /// so the rule is pinned by a test and not only by the implementation.
    #[test]
    fn the_register_folds_the_log() {
        let mut log = Log::default();
        assert_eq!(log.register(), Digest::zero(), "nothing measured is zero");

        log.text("cmdline", "root=/dev/vda rw");
        let e = log.events()[0].extension();
        let mut h = Sha384::new();
        h.update(Digest::zero().0);
        h.update(e.0);
        assert_eq!(log.register().hex(), hex(&h.finalize()));
    }

    /// An event's name is inside its extension, so the same bytes under a
    /// different name are a different measurement.
    #[test]
    fn the_name_is_part_of_the_event() {
        let mut a = Log::default();
        a.text("kernel", "same bytes");
        let mut b = Log::default();
        b.text("initrd", "same bytes");
        assert_ne!(a.register(), b.register());
    }

    /// Order is committed to: swapping two events changes the register.
    #[test]
    fn the_register_commits_to_order() {
        let mut a = Log::default();
        a.text("kernel", "K");
        a.text("root", "R");
        let mut b = Log::default();
        b.text("root", "R");
        b.text("kernel", "K");
        assert_ne!(a.register(), b.register());
    }

    /// A prefix is not the whole: appending always moves the register.
    #[test]
    fn extending_moves_the_register() {
        let mut log = Log::default();
        log.text("kernel", "K");
        let before = log.register();
        log.text("root", "R");
        assert_ne!(before, log.register());
    }

    /// Bytes measure as their content; where they came from does not enter
    /// the digest, so text and the same text as bytes are one event.
    #[test]
    fn bytes_measure_their_content_not_their_source() {
        let mut a = Log::default();
        a.bytes(
            "manifest",
            "/var/lib/rancher/k3s/server/manifests/cloud.yaml",
            b"kind: Deployment",
        );
        let mut b = Log::default();
        b.bytes("manifest", "somewhere else entirely", b"kind: Deployment");
        assert_eq!(a.register(), b.register());
        assert_ne!(a.events()[0].source, b.events()[0].source);

        let mut t = Log::default();
        t.text("cmdline", "root=/dev/vda");
        let mut y = Log::default();
        y.bytes("cmdline", "root=/dev/vda", b"root=/dev/vda");
        assert_eq!(t.events(), y.events());
    }

    /// The path a file came from is not measured — the same content at two
    /// paths is one measurement, so a measurement is reproducible off-host.
    #[test]
    fn the_path_is_not_measured() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("one"), dir.path().join("two"));
        std::fs::write(&a, b"identical").unwrap();
        std::fs::write(&b, b"identical").unwrap();

        let mut cache = Cache::none();
        let mut first = Log::default();
        first.file("root", &a, &mut cache).unwrap();
        let mut second = Log::default();
        second.file("root", &b, &mut cache).unwrap();

        assert_eq!(first.register(), second.register());
        assert_ne!(first.events()[0].source, second.events()[0].source);
    }

    #[test]
    fn a_file_measures_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Image");
        // Two buffers' worth plus a tail, so the streaming read is exercised.
        let bytes: Vec<u8> = (0..(2 * (1 << 20) + 7)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(digest_file(&path).unwrap(), Digest::of(&bytes));
    }

    /// The cache answers from the file's identity, and recomputes when the
    /// file behind the path changes.
    #[test]
    fn the_cache_follows_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rootfs.ext4");
        std::fs::write(&path, b"first").unwrap();

        let mut cache = Cache::open(dir.path().join("digests.json"));
        let first = cache.digest(&path).unwrap();
        assert_eq!(first, Digest::of(b"first"));
        assert_eq!(cache.digest(&path).unwrap(), first, "a hit is the same");
        cache.save();

        // A reopened cache still knows it, without touching the file's bytes.
        let mut reopened = Cache::open(dir.path().join("digests.json"));
        assert_eq!(reopened.digest(&path).unwrap(), first);

        std::fs::write(&path, b"second image").unwrap();
        assert_eq!(reopened.digest(&path).unwrap(), Digest::of(b"second image"));
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_zero() {
        let mut cache = Cache::none();
        assert!(cache.digest(Path::new("/nonexistent/Image")).is_err());
    }

    /// The document carries its registers and its bind, and reading it back
    /// verifies both — an edited value is refused, not believed.
    #[test]
    fn a_measurement_round_trips_and_is_verified() {
        let mut m = Measurement::default();
        m.launch.text("cmdline", "root=/dev/vda rw quiet");
        m.workload.text("image", "ghcr.io/hanzoai/cloud@sha256:00");

        let json = m.to_json();
        assert_eq!(Measurement::from_json(&json).unwrap(), m);

        let tampered = json.replace(&m.launch.register().hex(), &Digest::of(b"lie").hex());
        assert!(Measurement::from_json(&tampered).is_err());

        let rebound = json.replace(&m.bind_hex(), &hex(&[7u8; 64]));
        let err = Measurement::from_json(&rebound).unwrap_err();
        assert!(err.to_string().contains("bind"), "{err}");
    }

    /// The bind is 64 bytes and moves with either half.
    #[test]
    fn the_bind_covers_both_registers() {
        let mut m = Measurement::default();
        m.launch.text("kernel", "K");
        let launch_only = m.bind();
        assert_eq!(launch_only.len(), 64);

        m.workload.text("image", "I");
        assert_ne!(m.bind(), launch_only);
    }

    /// A bind survives the wire that carries it to whatever asks a platform to
    /// sign, and anything that is not 64 bytes of hex is refused there rather
    /// than padded into a report over bytes nobody chose.
    #[test]
    fn a_bind_round_trips_through_hex() {
        let mut m = Measurement::default();
        m.launch.text("kernel", "K");
        assert_eq!(Measurement::parse_bind(&m.bind_hex()).unwrap(), m.bind());

        assert!(Measurement::parse_bind("").is_err());
        assert!(Measurement::parse_bind(&"ab".repeat(63)).is_err(), "short");
        assert!(Measurement::parse_bind(&"ab".repeat(65)).is_err(), "long");
        assert!(
            Measurement::parse_bind(&"zz".repeat(64)).is_err(),
            "not hex"
        );
    }
}
