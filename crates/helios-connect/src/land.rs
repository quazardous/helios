//! Where fetched bytes land (`docs/design/import-connectors.md`, sections
//! 2.2 and 2.5): a part file Helios owns, hashed while it is written, in
//! BLAKE3 (the image's identity) and, when the item claims a hash, in the
//! claim's algorithm too.
//!
//! A [`Landing`] opens a part file from an offset: the bytes already there
//! (a fetch cut halfway, in this run or an earlier one) are read once to
//! bring the hashers to that point, and the connector goes on from there.
//! [`Landing::finish`] says what landed: its size, its BLAKE3 and how the
//! claim fared ([`Check`]). Nothing is trusted until then.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha1::Digest as _;

use crate::{Claim, FetchSink, HashAlgo};

/// A hash in one algorithm, computed as bytes arrive.
enum Hasher {
    Blake3(Box<blake3::Hasher>),
    Md5(md5::Context),
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
}

impl Hasher {
    /// A hasher for `algo`; `None` for one Helios cannot compute.
    fn of(algo: &HashAlgo) -> Option<Self> {
        Some(match algo {
            HashAlgo::Blake3 => Self::Blake3(Box::default()),
            HashAlgo::Md5 => Self::Md5(md5::Context::new()),
            HashAlgo::Sha1 => Self::Sha1(sha1::Sha1::new()),
            HashAlgo::Sha256 => Self::Sha256(sha2::Sha256::new()),
            HashAlgo::Other(_) => return None,
        })
    }

    fn update(&mut self, chunk: &[u8]) {
        match self {
            Self::Blake3(h) => {
                h.update(chunk);
            }
            Self::Md5(h) => h.consume(chunk),
            Self::Sha1(h) => h.update(chunk),
            Self::Sha256(h) => h.update(chunk),
        }
    }

    /// The digest, in lower-case hexadecimal.
    fn hex(self) -> String {
        match self {
            Self::Blake3(h) => h.finalize().to_hex().to_string(),
            Self::Md5(h) => format!("{:x}", h.finalize()),
            Self::Sha1(h) => hex(&h.finalize()),
            Self::Sha256(h) => hex(&h.finalize()),
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// How an item's claim fared against the bytes received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// The item claims no hash.
    None,
    /// The bytes hash to the claim.
    Matched,
    /// They do not: what was computed.
    Mismatch { computed: String },
    /// An algorithm Helios cannot compute: recorded, reported unchecked.
    Unchecked,
}

/// What landed in a part file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landed {
    pub path: PathBuf,
    pub bytes: u64,
    /// The BLAKE3 of the whole file: the image's identity.
    pub blake3: [u8; 32],
    pub check: Check,
}

/// A part file being written: the [`FetchSink`] Helios hands a connector.
pub struct Landing {
    path: PathBuf,
    file: File,
    blake3: blake3::Hasher,
    claim: Option<(Claim, Option<Hasher>)>,
    written: u64,
}

impl Landing {
    /// Opens `path` to go on from byte `offset`: what it holds up to there
    /// is kept and hashed again, anything past it dropped. `offset` 0
    /// starts afresh. The offset actually kept is [`Landing::written`]:
    /// less than asked when the file is shorter.
    pub fn open(path: &Path, offset: u64, claim: Option<&Claim>) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let held = file.metadata()?.len();
        let keep = offset.min(held);
        file.set_len(keep)?;
        let mut landing = Self {
            path: path.to_path_buf(),
            file: file.try_clone()?,
            blake3: blake3::Hasher::new(),
            claim: claim.map(|c| (c.clone(), Hasher::of(&c.algorithm))),
            written: 0,
        };
        // The bytes kept are read once, so the hashers stand where the
        // connector goes on.
        file.seek(SeekFrom::Start(0))?;
        let mut buffer = vec![0u8; 1 << 20];
        let mut from = (&mut file).take(keep);
        loop {
            let n = match from.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            landing.hash(&buffer[..n]);
        }
        landing.file.seek(SeekFrom::Start(keep))?;
        Ok(landing)
    }

    fn hash(&mut self, chunk: &[u8]) {
        self.blake3.update(chunk);
        if let Some((_, Some(h))) = &mut self.claim {
            h.update(chunk);
        }
        self.written += chunk.len() as u64;
    }

    /// The bytes in the part file so far.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Flushes the part to disk and says what landed.
    pub fn finish(self) -> std::io::Result<Landed> {
        self.file.sync_all()?;
        let check = match self.claim {
            None => Check::None,
            Some((_, None)) => Check::Unchecked,
            Some((claim, Some(h))) => {
                let computed = h.hex();
                if computed.eq_ignore_ascii_case(&claim.digest) {
                    Check::Matched
                } else {
                    Check::Mismatch { computed }
                }
            }
        };
        Ok(Landed {
            path: self.path,
            bytes: self.written,
            blake3: *self.blake3.finalize().as_bytes(),
            check,
        })
    }
}

impl FetchSink for Landing {
    fn accept(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        self.file.write_all(chunk)?;
        self.hash(chunk);
        Ok(())
    }
}

/// The digest of `bytes` in `algo`, in lower-case hexadecimal; `None` for
/// an algorithm Helios cannot compute. For claims of files already here.
pub fn digest(algo: &HashAlgo, bytes: &[u8]) -> Option<String> {
    let mut h = Hasher::of(algo)?;
    h.update(bytes);
    Some(h.hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("helios-land-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    const ABC_MD5: &str = "900150983cd24fb0d6963f7d28e17f72";
    const ABC_SHA1: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn the_known_digests_of_abc() {
        assert_eq!(digest(&HashAlgo::Md5, b"abc").unwrap(), ABC_MD5);
        assert_eq!(digest(&HashAlgo::Sha1, b"abc").unwrap(), ABC_SHA1);
        assert_eq!(digest(&HashAlgo::Sha256, b"abc").unwrap(), ABC_SHA256);
        assert_eq!(
            digest(&HashAlgo::Blake3, b"abc").unwrap(),
            blake3::hash(b"abc").to_hex().to_string()
        );
        assert_eq!(digest(&HashAlgo::Other("dropbox".into()), b"abc"), None);
    }

    #[test]
    fn a_landing_resumed_halfway_hashes_the_whole_file() {
        let dir = scratch("resume");
        let part = dir.join("x.part");
        let claim = Claim {
            algorithm: HashAlgo::Sha256,
            digest: ABC_SHA256.to_uppercase(),
        };
        let mut first = Landing::open(&part, 0, Some(&claim)).unwrap();
        first.accept(b"ab").unwrap();
        drop(first);
        // A later landing goes on from byte 2: what is past it is dropped.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .unwrap()
            .write_all(b"zz")
            .unwrap();
        let mut again = Landing::open(&part, 2, Some(&claim)).unwrap();
        assert_eq!(again.written(), 2);
        again.accept(b"c").unwrap();
        let landed = again.finish().unwrap();
        assert_eq!(landed.bytes, 3);
        assert_eq!(landed.blake3, *blake3::hash(b"abc").as_bytes());
        assert_eq!(landed.check, Check::Matched);
        assert_eq!(std::fs::read(&part).unwrap(), b"abc");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_claim_is_a_mismatch_and_an_unknown_one_unchecked() {
        let dir = scratch("claims");
        let part = dir.join("x.part");
        let wrong = Claim {
            algorithm: HashAlgo::Md5,
            digest: "0".repeat(32),
        };
        let mut landing = Landing::open(&part, 0, Some(&wrong)).unwrap();
        landing.accept(b"abc").unwrap();
        assert_eq!(
            landing.finish().unwrap().check,
            Check::Mismatch {
                computed: ABC_MD5.into()
            }
        );
        let other = Claim {
            algorithm: HashAlgo::Other("dropbox".into()),
            digest: "x".into(),
        };
        let mut landing = Landing::open(&part, 0, Some(&other)).unwrap();
        landing.accept(b"abc").unwrap();
        let landed = landing.finish().unwrap();
        assert_eq!((landed.bytes, landed.check), (3, Check::Unchecked));
        let mut landing = Landing::open(&part, 0, None).unwrap();
        landing.accept(b"abc").unwrap();
        assert_eq!(landing.finish().unwrap().check, Check::None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
