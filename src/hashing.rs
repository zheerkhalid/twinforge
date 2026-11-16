//! Content hashing for the exact-duplicate stages: opening files for shared
//! sequential reading, hashing the two ends of a file, hashing a whole file
//! as a stream and comparing two files byte by byte. Read-only.

use crate::model::HashAlgo;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Size of each of the two blocks (start and end of the file) that the
/// partial hash covers.
pub const EDGE_BLOCK: usize = 16 * 1024;
/// Files up to this size are hashed whole straight away; a partial hash would
/// cover all of their bytes anyway.
pub const WHOLE_LIMIT: u64 = 2 * EDGE_BLOCK as u64;
/// Buffer size for streaming whole files.
pub const READ_BUFFER: usize = 1024 * 1024;

const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
/// Other programs may read, write, rename and delete the file while it is open here.
const SHARE_EVERYTHING: u32 = 0x1 | 0x2 | 0x4;

/// A content hash. BLAKE3 fills all 32 bytes; XXH3 fills the first 16 and
/// leaves the rest zero.
pub type Digest = [u8; 32];

/// How many bytes of a [`Digest`] carry information.
pub fn digest_len(algo: HashAlgo) -> usize {
    match algo {
        HashAlgo::Blake3 => 32,
        HashAlgo::Xxh3 => 16,
    }
}

/// The short name shown in group labels.
pub fn algo_name(algo: HashAlgo) -> &'static str {
    match algo {
        HashAlgo::Blake3 => "BLAKE3",
        HashAlgo::Xxh3 => "XXH3",
    }
}

/// Lower-case hex digits of the meaningful part of a digest.
pub fn hex(algo: HashAlgo, digest: &Digest) -> String {
    let mut out = String::with_capacity(64);
    for byte in &digest[..digest_len(algo)] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The label of a duplicate group, e.g. `BLAKE3 3fa9c1d2e4b7…`.
pub fn label(algo: HashAlgo, digest: &Digest) -> String {
    let digits = hex(algo, digest);
    format!("{} {}\u{2026}", algo_name(algo), &digits[..12])
}

/// An incremental hasher for either algorithm.
pub enum Hasher {
    Blake3(Box<blake3::Hasher>),
    Xxh3(Box<xxhash_rust::xxh3::Xxh3>),
}

impl Hasher {
    pub fn new(algo: HashAlgo) -> Hasher {
        match algo {
            HashAlgo::Blake3 => Hasher::Blake3(Box::new(blake3::Hasher::new())),
            HashAlgo::Xxh3 => Hasher::Xxh3(Box::new(xxhash_rust::xxh3::Xxh3::new())),
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        match self {
            Hasher::Blake3(h) => {
                h.update(data);
            }
            Hasher::Xxh3(h) => h.update(data),
        }
    }

    pub fn finish(&self) -> Digest {
        match self {
            Hasher::Blake3(h) => *h.finalize().as_bytes(),
            Hasher::Xxh3(h) => {
                let mut out = [0u8; 32];
                out[..16].copy_from_slice(&h.digest128().to_be_bytes());
                out
            }
        }
    }
}

/// Hashes a block of memory.
pub fn hash_bytes(algo: HashAlgo, data: &[u8]) -> Digest {
    let mut hasher = Hasher::new(algo);
    hasher.update(data);
    hasher.finish()
}

/// Opens a file for reading from start to end without getting in the way of
/// other programs.
pub fn open_shared(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_EVERYTHING)
        .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN)
        .open(path)
}

/// Size and last write time of an open file. Two equal stamps taken before
/// and after reading mean that the file was not changed in between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub size: u64,
    /// Last write time in 100 ns steps since 1601 (the precision of NTFS).
    pub modified: u64,
}

pub fn stamp(file: &File) -> io::Result<Stamp> {
    let meta = file.metadata()?;
    Ok(Stamp {
        size: meta.file_size(),
        modified: meta.last_write_time(),
    })
}

/// Reads until the buffer is full or the file ends; returns the bytes read.
fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// The partial hash: the first and the last [`EDGE_BLOCK`] bytes of a file
/// that is larger than [`WHOLE_LIMIT`]. `buf` must hold at least one block.
pub fn hash_edges(
    file: &mut File,
    size: u64,
    algo: HashAlgo,
    buf: &mut [u8],
) -> io::Result<Digest> {
    if size <= WHOLE_LIMIT || buf.len() < EDGE_BLOCK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the partial hash needs a file larger than two blocks",
        ));
    }
    let block = &mut buf[..EDGE_BLOCK];
    let mut hasher = Hasher::new(algo);
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(block)?;
    hasher.update(block);
    file.seek(SeekFrom::Start(size - EDGE_BLOCK as u64))?;
    file.read_exact(block)?;
    hasher.update(block);
    Ok(hasher.finish())
}

/// Hashes a file from its current position to its end. `tick` is called with
/// the size of every chunk read and returns `false` to stop early.
/// Returns the digest and the number of bytes read, or `None` when stopped.
pub fn hash_stream(
    file: &mut File,
    algo: HashAlgo,
    buf: &mut [u8],
    mut tick: impl FnMut(u64) -> bool,
) -> io::Result<Option<(Digest, u64)>> {
    let mut hasher = Hasher::new(algo);
    let mut total = 0u64;
    loop {
        let n = read_full(file, buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
        if !tick(n as u64) {
            return Ok(None);
        }
        if n < buf.len() {
            break;
        }
    }
    Ok(Some((hasher.finish(), total)))
}

/// Which of the two files of a comparison could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    First,
    Second,
}

/// Compares two open files byte by byte from their current positions. `tick`
/// is called with the number of bytes read from each file per round and
/// returns `false` to stop early. `Ok(None)` means stopped.
pub fn same_content(
    first: &mut File,
    second: &mut File,
    buf_first: &mut [u8],
    buf_second: &mut [u8],
    mut tick: impl FnMut(u64) -> bool,
) -> Result<Option<bool>, (Side, io::Error)> {
    let len = buf_first.len().min(buf_second.len());
    loop {
        let a = read_full(first, &mut buf_first[..len]).map_err(|e| (Side::First, e))?;
        let b = read_full(second, &mut buf_second[..len]).map_err(|e| (Side::Second, e))?;
        let go_on = tick(a.max(b) as u64);
        if a != b || buf_first[..a] != buf_second[..b] {
            return Ok(Some(false));
        }
        if a < len {
            return Ok(Some(true));
        }
        if !go_on {
            return Ok(None);
        }
    }
}

/// Helpers shared by the unit tests of the hashing, cache, duplicate and
/// small-finder modules. Everything happens inside throw-away temp folders.
#[cfg(test)]
pub(crate) mod testkit {
    use crate::model::ScanResult;
    use crate::util::file_name;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    /// A fresh, empty folder inside the temp folder.
    pub fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("twinforge-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Deterministic filler bytes; different for every seed.
    pub fn filler(seed: u64, len: usize) -> Vec<u8> {
        let mut x = seed.wrapping_mul(0x2545_F491_4F6C_DD1D) | 1;
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            out.extend_from_slice(&x.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
        }
        out.truncate(len);
        out
    }

    /// Writes a file, creating its folder first.
    pub fn put(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    /// Sets the last write time of a file to `secs_ago` seconds in the past.
    pub fn age(path: &Path, secs_ago: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
            .unwrap();
    }

    /// Writes a file that looks as if it was last changed `secs_ago` seconds ago.
    pub fn put_aged(path: &Path, bytes: &[u8], secs_ago: u64) {
        put(path, bytes);
        age(path, secs_ago);
    }

    /// The groups of a result as sorted lists of file names, themselves sorted.
    pub fn shape(result: &ScanResult) -> Vec<Vec<String>> {
        let mut groups: Vec<Vec<String>> = result
            .groups
            .iter()
            .map(|g| {
                let mut names: Vec<String> =
                    g.items.iter().map(|i| file_name(&i.file.path)).collect();
                names.sort();
                names
            })
            .collect();
        groups.sort();
        groups
    }

    /// The file names of a flat result in the order the finder returned them.
    pub fn flat_names(result: &ScanResult) -> Vec<String> {
        result
            .groups
            .iter()
            .map(|g| {
                assert_eq!(g.items.len(), 1, "flat finders return one item per group");
                file_name(&g.items[0].file.path)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{filler, put, temp_dir};
    use super::*;
    use std::fs;

    #[test]
    fn known_answers() {
        assert_eq!(
            hex(HashAlgo::Blake3, &hash_bytes(HashAlgo::Blake3, b"")),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(
            hex(HashAlgo::Blake3, &hash_bytes(HashAlgo::Blake3, b"abc")),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert_eq!(
            hex(HashAlgo::Xxh3, &hash_bytes(HashAlgo::Xxh3, b"")),
            "99aa06d3014798d86001c324468d497f"
        );
        // The streaming and the one-shot XXH3 agree, and the unused half stays zero.
        let data = filler(4, 70_000);
        let digest = hash_bytes(HashAlgo::Xxh3, &data);
        assert_eq!(
            digest[..16],
            xxhash_rust::xxh3::xxh3_128(&data).to_be_bytes()
        );
        assert_eq!(digest[16..], [0u8; 16]);
        assert_eq!(hex(HashAlgo::Xxh3, &digest).len(), 32);
    }

    #[test]
    fn labels_show_twelve_digits() {
        let digest = hash_bytes(HashAlgo::Blake3, b"");
        assert_eq!(
            label(HashAlgo::Blake3, &digest),
            "BLAKE3 af1349b9f5f9\u{2026}"
        );
        let digest = hash_bytes(HashAlgo::Xxh3, b"");
        assert_eq!(label(HashAlgo::Xxh3, &digest), "XXH3 99aa06d30147\u{2026}");
    }

    #[test]
    fn stream_matches_memory_for_any_buffer_size() {
        let dir = temp_dir("hashing-stream");
        let data = filler(9, 300_123);
        let path = dir.join("blob.bin");
        put(&path, &data);
        for algo in [HashAlgo::Blake3, HashAlgo::Xxh3] {
            for buf_len in [1000usize, 65_536, READ_BUFFER] {
                let mut buf = vec![0u8; buf_len];
                let mut file = open_shared(&path).unwrap();
                let mut ticks = 0u64;
                let (digest, total) = hash_stream(&mut file, algo, &mut buf, |n| {
                    ticks += n;
                    true
                })
                .unwrap()
                .unwrap();
                assert_eq!(digest, hash_bytes(algo, &data));
                assert_eq!((total, ticks), (data.len() as u64, data.len() as u64));
            }
        }
        // Stopping early gives no digest.
        let mut file = open_shared(&path).unwrap();
        let mut buf = vec![0u8; 1000];
        let stopped = hash_stream(&mut file, HashAlgo::Blake3, &mut buf, |_| false).unwrap();
        assert!(stopped.is_none());
        assert_eq!(stamp(&file).unwrap().size, data.len() as u64);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edges_cover_exactly_the_first_and_last_block() {
        let dir = temp_dir("hashing-edges");
        let data = filler(2, 200_000);
        let mut middle = data.clone();
        middle[100_000] ^= 1;
        let mut head = data.clone();
        head[EDGE_BLOCK - 1] ^= 1;
        let mut tail = data.clone();
        tail[200_000 - EDGE_BLOCK] ^= 1;
        let mut buf = vec![0u8; EDGE_BLOCK];
        let mut edges = Vec::new();
        for (name, bytes) in [("a", &data), ("m", &middle), ("h", &head), ("t", &tail)] {
            let path = dir.join(name);
            put(&path, bytes);
            let mut file = open_shared(&path).unwrap();
            edges.push(hash_edges(&mut file, 200_000, HashAlgo::Blake3, &mut buf).unwrap());
        }
        let mut both = data[..EDGE_BLOCK].to_vec();
        both.extend_from_slice(&data[200_000 - EDGE_BLOCK..]);
        assert_eq!(edges[0], hash_bytes(HashAlgo::Blake3, &both));
        assert_eq!(
            edges[0], edges[1],
            "the middle is not part of the partial hash"
        );
        assert_ne!(edges[0], edges[2]);
        assert_ne!(edges[0], edges[3]);
        // Small files have no partial hash.
        put(&dir.join("small"), &data[..1000]);
        let mut file = open_shared(&dir.join("small")).unwrap();
        assert!(hash_edges(&mut file, 1000, HashAlgo::Blake3, &mut buf).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn byte_comparison() {
        let dir = temp_dir("hashing-compare");
        let data = filler(5, 150_000);
        let mut last = data.clone();
        last[149_999] ^= 0x80;
        put(&dir.join("a"), &data);
        put(&dir.join("b"), &data);
        put(&dir.join("c"), &last);
        put(&dir.join("d"), &data[..149_000]);
        put(&dir.join("e"), b"");
        let compare = |x: &str, y: &str, buf_len: usize| {
            let mut first = open_shared(&dir.join(x)).unwrap();
            let mut second = open_shared(&dir.join(y)).unwrap();
            let (mut b1, mut b2) = (vec![0u8; buf_len], vec![0u8; buf_len]);
            same_content(&mut first, &mut second, &mut b1, &mut b2, |_| true).unwrap()
        };
        for buf_len in [4096usize, 150_000, READ_BUFFER] {
            assert_eq!(compare("a", "b", buf_len), Some(true));
            assert_eq!(compare("a", "c", buf_len), Some(false));
            assert_eq!(compare("a", "d", buf_len), Some(false));
            assert_eq!(compare("d", "a", buf_len), Some(false));
            assert_eq!(compare("e", "e", buf_len), Some(true));
        }
        let mut first = open_shared(&dir.join("a")).unwrap();
        let mut second = open_shared(&dir.join("b")).unwrap();
        let (mut b1, mut b2) = (vec![0u8; 4096], vec![0u8; 4096]);
        let stopped = same_content(&mut first, &mut second, &mut b1, &mut b2, |_| false).unwrap();
        assert_eq!(stopped, None);
        // The bytes of a short last round are reported as well.
        let mut first = open_shared(&dir.join("a")).unwrap();
        let mut second = open_shared(&dir.join("b")).unwrap();
        let mut seen = 0u64;
        let same = same_content(&mut first, &mut second, &mut b1, &mut b2, |n| {
            seen += n;
            true
        })
        .unwrap();
        assert_eq!((same, seen), (Some(true), 150_000));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_files_stay_usable_by_others() {
        let dir = temp_dir("hashing-share");
        let path = dir.join("busy.txt");
        put(&path, b"first");
        let held = open_shared(&path).unwrap();
        fs::write(&path, b"second").unwrap();
        fs::rename(&path, dir.join("renamed.txt")).unwrap();
        fs::remove_file(dir.join("renamed.txt")).unwrap();
        drop(held);
        fs::remove_dir_all(&dir).unwrap();
    }
}
