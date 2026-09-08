//! Chunks that outlive the host they were made on.
//!
//! A checkpoint is already content-addressed and chunked, so archiving one is
//! not a new format — it is the same chunks, somewhere durable. This is the
//! `ChunkStore` the trait in `cas` was written for, and a tier that puts a local
//! cache in front of it so a reload pays the network once per chunk and never
//! again.

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::cas::ChunkStore;

type HmacSha256 = Hmac<Sha256>;

/// Where the archive lives, and who may write to it.
///
/// Read from the environment because that is where every other S3 consumer in
/// the estate reads it from, under the names rclone and the AWS SDKs both
/// already use — one spelling, so a host configured for one is configured for
/// all of them.
#[derive(Clone, Debug)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Key prefix inside the bucket. Chunks land at `{prefix}/{hash}`.
    pub prefix: String,
}

impl S3Config {
    /// Reads the environment, returning None when no endpoint is set — which is
    /// "this host has no archive", not an error. A vm that never archives must
    /// not fail to start because nobody configured one.
    pub fn from_env() -> Option<Self> {
        let endpoint = env_any(&["HANZO_VM_S3_ENDPOINT", "S3_ENDPOINT", "AWS_ENDPOINT_URL"])?;
        Some(S3Config {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            bucket: env_any(&["HANZO_VM_S3_BUCKET", "S3_BUCKET"]).unwrap_or_else(|| "vm".into()),
            region: env_any(&["HANZO_VM_S3_REGION", "AWS_REGION"]).unwrap_or_else(|| "us-east-1".into()),
            access_key: env_any(&["HANZO_VM_S3_ACCESS_KEY", "AWS_ACCESS_KEY_ID"]).unwrap_or_default(),
            secret_key: env_any(&["HANZO_VM_S3_SECRET_KEY", "AWS_SECRET_ACCESS_KEY"]).unwrap_or_default(),
            prefix: env_any(&["HANZO_VM_S3_PREFIX"]).unwrap_or_else(|| "chunks".into()),
        })
    }
}

fn env_any(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// One chunk store backed by an S3-compatible endpoint.
pub struct S3ChunkStore {
    cfg: S3Config,
}

impl S3ChunkStore {
    pub fn new(cfg: S3Config) -> Self {
        S3ChunkStore { cfg }
    }

    pub fn from_env() -> Option<Self> {
        S3Config::from_env().map(Self::new)
    }

    fn url(&self, hash: &str) -> String {
        format!("{}/{}/{}/{}", self.cfg.endpoint, self.cfg.bucket, self.cfg.prefix, hash)
    }

    fn key(&self, hash: &str) -> String {
        format!("/{}/{}/{}", self.cfg.bucket, self.cfg.prefix, hash)
    }

    /// SigV4, the minimum of it: one header set, no query signing, no chunked
    /// upload. A chunk is 64KB and immutable, so the request this signs is
    /// always a whole-object PUT or GET.
    fn sign(&self, method: &str, key: &str, payload_sha: &str) -> Result<Vec<(String, String)>> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let (date, stamp) = (ymd(now), iso8601(now));
        let host = self
            .cfg
            .endpoint
            .split("://")
            .nth(1)
            .unwrap_or(&self.cfg.endpoint)
            .to_string();

        let canonical = format!(
            "{method}\n{key}\n\nhost:{host}\nx-amz-content-sha256:{payload_sha}\nx-amz-date:{stamp}\n\n\
             host;x-amz-content-sha256;x-amz-date\n{payload_sha}"
        );
        let scope = format!("{date}/{}/s3/aws4_request", self.cfg.region);
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );

        let mut k = hmac(format!("AWS4{}", self.cfg.secret_key).as_bytes(), date.as_bytes())?;
        k = hmac(&k, self.cfg.region.as_bytes())?;
        k = hmac(&k, b"s3")?;
        k = hmac(&k, b"aws4_request")?;
        let sig = hex(&hmac(&k, to_sign.as_bytes())?);

        Ok(vec![
            ("x-amz-date".into(), stamp),
            ("x-amz-content-sha256".into(), payload_sha.to_string()),
            (
                "authorization".into(),
                format!(
                    "AWS4-HMAC-SHA256 Credential={}/{scope}, \
                     SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature={sig}",
                    self.cfg.access_key
                ),
            ),
        ])
    }
}

impl ChunkStore for S3ChunkStore {
    fn put(&self, data: &[u8]) -> Result<String> {
        let hash = blake3::hash(data).to_hex().to_string();
        let payload = hex(&Sha256::digest(data));
        let mut req = ureq::put(&self.url(&hash));
        for (k, v) in self.sign("PUT", &self.key(&hash), &payload)? {
            req = req.header(&k, &v);
        }
        match req.send(data) {
            Ok(_) => Ok(hash),
            Err(e) => bail!("s3 put {hash}: {e}"),
        }
    }

    fn get(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        const EMPTY_SHA: &str =
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let mut req = ureq::get(&self.url(hash));
        for (k, v) in self.sign("GET", &self.key(hash), EMPTY_SHA)? {
            req = req.header(&k, &v);
        }
        match req.call() {
            Ok(resp) => {
                let mut buf = Vec::new();
                resp.into_body()
                    .into_reader()
                    .read_to_end(&mut buf)
                    .context("s3 get: read body")?;
                Ok(Some(buf))
            }
            // A chunk that is not there is not a failure: the caller falls back
            // to whatever else it has, which is the whole point of a tier.
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(e) => bail!("s3 get {hash}: {e}"),
        }
    }
}

/// Local first, remote behind it.
///
/// A reload reads from the archive ONCE per chunk and keeps it, so the second
/// resume of the same checkpoint costs nothing over a purely local one. Writes
/// go to both, because a chunk that exists only locally is not archived and a
/// chunk that exists only remotely makes every read a network round trip.
pub struct Tiered {
    near: Box<dyn ChunkStore>,
    far: Box<dyn ChunkStore>,
}

impl Tiered {
    pub fn new(near: Box<dyn ChunkStore>, far: Box<dyn ChunkStore>) -> Self {
        Tiered { near, far }
    }
}

impl ChunkStore for Tiered {
    fn put(&self, data: &[u8]) -> Result<String> {
        let hash = self.near.put(data)?;
        // The archive is durability, not correctness: a host that cannot reach
        // it still has a working local checkpoint, and the chunk is
        // content-addressed so a later put is the same bytes under the same
        // name. Losing the write here costs a re-upload, never a wrong read.
        if let Err(e) = self.far.put(data) {
            tracing::warn!(%hash, error = %e, "chunk not archived; it is local-only until the next put");
        }
        Ok(hash)
    }

    fn get(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        if let Some(d) = self.near.get(hash)? {
            return Ok(Some(d));
        }
        match self.far.get(hash)? {
            Some(d) => {
                // Populate on the way past, so this is the only time this chunk
                // costs the network.
                if let Err(e) = self.near.put(&d) {
                    tracing::warn!(%hash, error = %e, "chunk fetched but not cached locally");
                }
                Ok(Some(d))
            }
            None => Ok(None),
        }
    }
}

fn hmac(key: &[u8], msg: &[u8]) -> Result<Vec<u8>> {
    let mut m = HmacSha256::new_from_slice(key).context("hmac key")?;
    m.update(msg);
    Ok(m.finalize().into_bytes().to_vec())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// YYYYMMDD and ISO8601-basic, from a unix second. Written here rather than
/// pulled from a date crate because SigV4 needs exactly these two shapes and
/// nothing else in this crate needs a calendar.
fn ymd(secs: u64) -> String {
    let (y, m, d) = civil(secs / 86_400);
    format!("{y:04}{m:02}{d:02}")
}

fn iso8601(secs: u64) -> String {
    let (y, m, d) = civil(secs / 86_400);
    let t = secs % 86_400;
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", t / 3600, (t % 3600) / 60, t % 60)
}

/// Days since the epoch to a civil date. Howard Hinnant's algorithm.
fn civil(z: u64) -> (i64, u32, u32) {
    let z = z as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip_known_days() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_000), (2022, 1, 8));
        assert_eq!(ymd(1_700_000_000), "20231114");
        assert_eq!(iso8601(1_700_000_000), "20231114T221320Z");
    }

    #[test]
    fn no_endpoint_means_no_archive_rather_than_an_error() {
        for k in ["HANZO_VM_S3_ENDPOINT", "S3_ENDPOINT", "AWS_ENDPOINT_URL"] {
            std::env::remove_var(k);
        }
        assert!(S3Config::from_env().is_none());
    }
}
