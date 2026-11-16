//! The hash cache: remembers content hashes between runs so that files which
//! have not changed are not read again.
//!
//! An entry belongs to one path and is only valid for the exact size and last
//! write time it was recorded with. The cache file is the only thing the scan
//! engine ever writes; it is replaced as a whole (temp file, then rename) and
//! carries a checksum, so a damaged file simply counts as an empty cache.

use crate::hashing::{digest_len, Digest, Stamp};
use crate::model::HashAlgo;
use crate::util::norm_path;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"TWFHASH\0";
const VERSION: u32 = 1;
/// Magic, version, entry count and the trailing checksum.
const FRAME: usize = 8 + 4 + 8 + 32;
/// Upper limit of remembered files; the least recently used are dropped first.
pub const MAX_ENTRIES: usize = 1_000_000;

const TICKS_PER_SEC: u64 = 10_000_000;
/// 1970-01-01 expressed in 100 ns steps since 1601.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
/// A whole-second time stamp may come from a file system or a tool that only
/// keeps seconds (FAT even rounds to two): such a file has to be unchanged
/// for this long before its hash is remembered or believed.
const COARSE_WINDOW: u64 = 2 * TICKS_PER_SEC;
/// A precise time stamp only repeats for writes within the same clock tick
/// (at most about 16 ms on Windows).
const FINE_WINDOW: u64 = TICKS_PER_SEC / 25;
/// "Last used" is only refreshed on disk when it is at least this stale.
const TOUCH_SECS: u64 = 24 * 3600;

/// Which hash of a file an entry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// First and last block only.
    Partial,
    /// The whole content.
    Full,
}

const SLOTS: usize = 4;

fn slot(algo: HashAlgo, kind: Kind) -> usize {
    let a = match algo {
        HashAlgo::Blake3 => 0,
        HashAlgo::Xxh3 => 1,
    };
    let k = match kind {
        Kind::Partial => 0,
        Kind::Full => 1,
    };
    a * 2 + k
}

fn slot_len(slot: usize) -> usize {
    digest_len(if slot < 2 {
        HashAlgo::Blake3
    } else {
        HashAlgo::Xxh3
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    size: u64,
    modified: u64,
    /// When the entry was last useful, seconds since 1970.
    used: u64,
    /// Bit `n` is set when `digests[n]` is filled.
    present: u8,
    digests: [Digest; SLOTS],
}

fn now_ticks() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| UNIX_EPOCH_TICKS + (d.as_nanos() / 100) as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Is a file with this last write time (100 ns steps since 1601) old enough
/// for its time stamp to prove that it has not changed since it was hashed?
/// A file written again within the resolution of its time stamp would look
/// unchanged, so very fresh files are always read from disk.
pub fn settled(modified: u64, now: u64) -> bool {
    if modified == 0 || modified > now {
        return false;
    }
    let window = if modified.is_multiple_of(TICKS_PER_SEC) {
        COARSE_WINDOW
    } else {
        FINE_WINDOW
    };
    now - modified >= window
}

/// The cache of one scan. Shared by the worker threads.
pub struct Cache {
    file: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
    dirty: AtomicBool,
}

impl Cache {
    /// Loads the cache file. A missing, damaged or foreign file gives an empty cache.
    pub fn open(file: &Path) -> Cache {
        let entries = std::fs::read(file)
            .ok()
            .and_then(|bytes| decode(&bytes))
            .unwrap_or_default();
        Cache {
            file: file.to_path_buf(),
            entries: Mutex::new(entries),
            dirty: AtomicBool::new(false),
        }
    }

    /// Number of remembered files.
    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The remembered hash of a file, if its size and time are exactly as recorded.
    pub fn get(&self, path: &Path, stamp: Stamp, algo: HashAlgo, kind: Kind) -> Option<Digest> {
        self.get_at(path, stamp, algo, kind, now_ticks())
    }

    fn get_at(
        &self,
        path: &Path,
        stamp: Stamp,
        algo: HashAlgo,
        kind: Kind,
        now: u64,
    ) -> Option<Digest> {
        if !settled(stamp.modified, now) {
            return None;
        }
        let key = norm_path(&path.to_string_lossy());
        let slot = slot(algo, kind);
        let mut entries = self.entries.lock().ok()?;
        let entry = entries.get_mut(&key)?;
        if entry.size != stamp.size
            || entry.modified != stamp.modified
            || entry.present & (1 << slot) == 0
        {
            return None;
        }
        let secs = now_secs();
        if secs.saturating_sub(entry.used) >= TOUCH_SECS {
            entry.used = secs;
            self.dirty.store(true, Ordering::Relaxed);
        }
        Some(entry.digests[slot])
    }

    /// Remembers a hash. `stamp` must have been taken from the open file and
    /// found unchanged after reading.
    pub fn put(&self, path: &Path, stamp: Stamp, algo: HashAlgo, kind: Kind, digest: Digest) {
        self.put_at(path, stamp, algo, kind, digest, now_ticks());
    }

    fn put_at(
        &self,
        path: &Path,
        stamp: Stamp,
        algo: HashAlgo,
        kind: Kind,
        digest: Digest,
        now: u64,
    ) {
        if !settled(stamp.modified, now) {
            return;
        }
        let key = norm_path(&path.to_string_lossy());
        let slot = slot(algo, kind);
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let entry = entries.entry(key).or_insert(Entry {
            size: stamp.size,
            modified: stamp.modified,
            used: 0,
            present: 0,
            digests: [[0u8; 32]; SLOTS],
        });
        if entry.size != stamp.size || entry.modified != stamp.modified {
            // The file changed: nothing recorded earlier is valid any more.
            entry.size = stamp.size;
            entry.modified = stamp.modified;
            entry.present = 0;
        }
        entry.used = now_secs();
        entry.present |= 1 << slot;
        entry.digests[slot] = digest;
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Writes the cache back if anything was learned: to a temp file next to
    /// the cache file, which then replaces it.
    pub fn save(&self) -> Result<(), String> {
        if !self.dirty.load(Ordering::Relaxed) {
            return Ok(());
        }
        let bytes = {
            let entries = self
                .entries
                .lock()
                .map_err(|_| "The hash cache could not be saved.".to_string())?;
            encode(&entries, MAX_ENTRIES)
        };
        let explain = |e: std::io::Error| format!("The hash cache could not be saved: {e}");
        if let Some(parent) = self.file.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(explain)?;
            }
        }
        let mut temp = self.file.clone().into_os_string();
        temp.push(format!(".{}.tmp", std::process::id()));
        let temp = PathBuf::from(temp);
        let written =
            std::fs::write(&temp, &bytes).and_then(|_| std::fs::rename(&temp, &self.file));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(explain(e));
        }
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }
}

fn encode(entries: &HashMap<String, Entry>, limit: usize) -> Vec<u8> {
    let mut list: Vec<(&String, &Entry)> = entries.iter().filter(|(_, e)| e.present != 0).collect();
    if list.len() > limit {
        // Most recently used first; the path makes the choice stable.
        list.sort_by(|a, b| b.1.used.cmp(&a.1.used).then_with(|| a.0.cmp(b.0)));
        list.truncate(limit);
    }
    list.sort_by(|a, b| a.0.cmp(b.0));
    let mut out = Vec::with_capacity(FRAME + list.len() * 160);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(list.len() as u64).to_le_bytes());
    for (path, entry) in list {
        out.extend_from_slice(&(path.len() as u32).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
        out.extend_from_slice(&entry.modified.to_le_bytes());
        out.extend_from_slice(&entry.used.to_le_bytes());
        out.push(entry.present);
        for slot in 0..SLOTS {
            if entry.present & (1 << slot) != 0 {
                out.extend_from_slice(&entry.digests[slot][..slot_len(slot)]);
            }
        }
    }
    let checksum = blake3::hash(&out);
    out.extend_from_slice(checksum.as_bytes());
    out
}

/// A cursor over the bytes of a cache file; every read is bounds-checked.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let part = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(part)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

fn decode(bytes: &[u8]) -> Option<HashMap<String, Entry>> {
    if bytes.len() < FRAME {
        return None;
    }
    let (body, checksum) = bytes.split_at(bytes.len() - 32);
    let mut reader = Reader { bytes: body, at: 0 };
    if reader.take(8)? != MAGIC || reader.u32()? != VERSION {
        return None;
    }
    if blake3::hash(body).as_bytes() != checksum {
        return None;
    }
    let count = reader.u64()?;
    // Every entry takes at least 29 bytes; a larger count cannot be honest.
    if count > (body.len() / 29) as u64 {
        return None;
    }
    let mut entries = HashMap::with_capacity(count as usize);
    for _ in 0..count {
        let path_len = reader.u32()? as usize;
        let path = std::str::from_utf8(reader.take(path_len)?)
            .ok()?
            .to_string();
        let size = reader.u64()?;
        let modified = reader.u64()?;
        let used = reader.u64()?;
        let present = *reader.take(1)?.first()?;
        if present >> SLOTS != 0 {
            return None;
        }
        let mut digests = [[0u8; 32]; SLOTS];
        for (slot, digest) in digests.iter_mut().enumerate() {
            if present & (1 << slot) != 0 {
                let part = reader.take(slot_len(slot))?;
                digest[..part.len()].copy_from_slice(part);
            }
        }
        entries.insert(
            path,
            Entry {
                size,
                modified,
                used,
                present,
                digests,
            },
        );
    }
    if reader.at != body.len() {
        return None;
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashing::hash_bytes;
    use crate::hashing::testkit::temp_dir;
    use std::fs;

    /// A time stamp with a sub-second part, one hour old.
    fn old_fine() -> u64 {
        (now_ticks() - 3600 * TICKS_PER_SEC) / TICKS_PER_SEC * TICKS_PER_SEC + 1_234_567
    }

    fn stamp(size: u64, modified: u64) -> Stamp {
        Stamp { size, modified }
    }

    #[test]
    fn fresh_files_are_not_trusted() {
        let now = 133_000_000_000_000_000u64 + 5_000_000;
        let whole = 133_000_000_000_000_000u64;
        // Whole-second stamps need two quiet seconds.
        assert!(!settled(whole, now));
        assert!(!settled(whole, whole + COARSE_WINDOW - 1));
        assert!(settled(whole, whole + COARSE_WINDOW));
        // Precise stamps need a few clock ticks.
        let fine = whole + 77;
        assert!(!settled(fine, fine));
        assert!(!settled(fine, fine + FINE_WINDOW - 1));
        assert!(settled(fine, fine + FINE_WINDOW));
        // Unknown or future times are never trusted.
        assert!(!settled(0, now));
        assert!(!settled(now + 1, now));

        let dir = temp_dir("cache-fresh");
        let cache = Cache::open(&dir.join("c.bin"));
        let digest = hash_bytes(HashAlgo::Blake3, b"x");
        let path = Path::new("Q:\\made-up\\fresh.bin");
        cache.put_at(
            path,
            stamp(1, whole),
            HashAlgo::Blake3,
            Kind::Full,
            digest,
            now,
        );
        assert!(
            cache.is_empty(),
            "a file changed half a second ago is not stored"
        );
        let later = whole + COARSE_WINDOW;
        cache.put_at(
            path,
            stamp(1, whole),
            HashAlgo::Blake3,
            Kind::Full,
            digest,
            later,
        );
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache.get_at(path, stamp(1, whole), HashAlgo::Blake3, Kind::Full, later),
            Some(digest)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keys_cover_path_size_time_algorithm_and_kind() {
        let dir = temp_dir("cache-keys");
        let cache = Cache::open(&dir.join("c.bin"));
        let t = old_fine();
        let path = Path::new("Q:\\Made-Up\\Folder\\Blob.bin");
        let full = hash_bytes(HashAlgo::Blake3, b"full");
        let part = hash_bytes(HashAlgo::Blake3, b"part");
        let xx = hash_bytes(HashAlgo::Xxh3, b"full");
        cache.put(path, stamp(500, t), HashAlgo::Blake3, Kind::Full, full);
        cache.put(path, stamp(500, t), HashAlgo::Blake3, Kind::Partial, part);
        cache.put(path, stamp(500, t), HashAlgo::Xxh3, Kind::Full, xx);
        assert_eq!(cache.len(), 1, "one entry per file");
        let get = |p: &str, size, time, algo, kind| {
            cache.get(Path::new(p), stamp(size, time), algo, kind)
        };
        let b3 = HashAlgo::Blake3;
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 500, t, b3, Kind::Full),
            Some(full)
        );
        assert_eq!(
            get("q:/made-up/folder/BLOB.BIN", 500, t, b3, Kind::Full),
            Some(full)
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 500, t, b3, Kind::Partial),
            Some(part)
        );
        assert_eq!(
            get(
                "Q:\\Made-Up\\Folder\\Blob.bin",
                500,
                t,
                HashAlgo::Xxh3,
                Kind::Full
            ),
            Some(xx)
        );
        assert_eq!(
            get(
                "Q:\\Made-Up\\Folder\\Blob.bin",
                500,
                t,
                HashAlgo::Xxh3,
                Kind::Partial
            ),
            None
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 501, t, b3, Kind::Full),
            None
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 500, t + 1, b3, Kind::Full),
            None
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Other.bin", 500, t, b3, Kind::Full),
            None
        );

        // A new stamp for the same path wipes everything recorded before.
        cache.put(path, stamp(500, t + 9), b3, Kind::Partial, part);
        assert_eq!(
            get(
                "Q:\\Made-Up\\Folder\\Blob.bin",
                500,
                t + 9,
                b3,
                Kind::Partial
            ),
            Some(part)
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 500, t + 9, b3, Kind::Full),
            None
        );
        assert_eq!(
            get("Q:\\Made-Up\\Folder\\Blob.bin", 500, t, b3, Kind::Full),
            None
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saves_and_loads_and_keeps_other_entries() {
        let dir = temp_dir("cache-roundtrip");
        let file = dir.join("deep\\er\\cache.bin");
        let t = old_fine();
        let a = hash_bytes(HashAlgo::Blake3, b"a");
        let b = hash_bytes(HashAlgo::Xxh3, b"b");
        let cache = Cache::open(&file);
        assert!(cache.is_empty());
        cache.save().unwrap();
        assert!(!file.exists(), "nothing learned, nothing written");
        cache.put(
            Path::new("Q:\\one\\a.bin"),
            stamp(10, t),
            HashAlgo::Blake3,
            Kind::Full,
            a,
        );
        cache.put(
            Path::new("Q:\\two\\b.bin"),
            stamp(20, t),
            HashAlgo::Xxh3,
            Kind::Partial,
            b,
        );
        cache.save().unwrap();
        assert!(file.exists(), "the parent folders were created");
        let leftovers: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, ["cache.bin"], "the temp file is gone");

        // A later run that only sees one folder keeps what is known about the other.
        let again = Cache::open(&file);
        assert_eq!(again.len(), 2);
        let c = hash_bytes(HashAlgo::Blake3, b"c");
        again.put(
            Path::new("Q:\\one\\c.bin"),
            stamp(30, t),
            HashAlgo::Blake3,
            Kind::Full,
            c,
        );
        again.save().unwrap();
        let third = Cache::open(&file);
        assert_eq!(third.len(), 3);
        let get = |p: &str, size, algo, kind| third.get(Path::new(p), stamp(size, t), algo, kind);
        assert_eq!(
            get("Q:\\one\\a.bin", 10, HashAlgo::Blake3, Kind::Full),
            Some(a)
        );
        assert_eq!(
            get("Q:\\two\\b.bin", 20, HashAlgo::Xxh3, Kind::Partial),
            Some(b)
        );
        assert_eq!(
            get("Q:\\one\\c.bin", 30, HashAlgo::Blake3, Kind::Full),
            Some(c)
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn damaged_files_count_as_empty() {
        let dir = temp_dir("cache-damaged");
        let file = dir.join("cache.bin");
        let t = old_fine();
        let cache = Cache::open(&file);
        for n in 0..20u64 {
            let digest = hash_bytes(HashAlgo::Blake3, &n.to_le_bytes());
            let path = format!("Q:\\made-up\\file-{n}.bin");
            cache.put(
                Path::new(&path),
                stamp(n + 1, t),
                HashAlgo::Blake3,
                Kind::Full,
                digest,
            );
        }
        cache.save().unwrap();
        let good = fs::read(&file).unwrap();
        assert_eq!(decode(&good).map(|e| e.len()), Some(20));

        assert!(decode(b"").is_none());
        assert!(decode(b"not a cache file at all, just some text that is long enough").is_none());
        for cut in [1usize, 31, 32, 33, good.len() / 2, good.len() - 1] {
            assert!(decode(&good[..good.len() - cut]).is_none(), "cut {cut}");
        }
        // One flipped bit anywhere is noticed.
        for at in [
            0usize,
            9,
            14,
            40,
            good.len() / 2,
            good.len() - 40,
            good.len() - 1,
        ] {
            let mut bad = good.clone();
            bad[at] ^= 0x10;
            assert!(decode(&bad).is_none(), "flip at {at}");
        }
        // A future version with a valid checksum is not understood either.
        let mut newer = good[..good.len() - 32].to_vec();
        newer[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        let sum = blake3::hash(&newer);
        newer.extend_from_slice(sum.as_bytes());
        assert!(decode(&newer).is_none());
        // Trailing rubbish after the entries, even with a valid checksum.
        let mut longer = good[..good.len() - 32].to_vec();
        longer.extend_from_slice(&[0u8; 7]);
        let sum = blake3::hash(&longer);
        longer.extend_from_slice(sum.as_bytes());
        assert!(decode(&longer).is_none());

        fs::write(&file, &good[..good.len() / 2]).unwrap();
        let broken = Cache::open(&file);
        assert!(broken.is_empty());
        // The damaged file is replaced by a good one on the next save.
        let d = hash_bytes(HashAlgo::Blake3, b"d");
        broken.put(
            Path::new("Q:\\made-up\\d.bin"),
            stamp(4, t),
            HashAlgo::Blake3,
            Kind::Full,
            d,
        );
        broken.save().unwrap();
        assert_eq!(Cache::open(&file).len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_oldest_entries_are_dropped_at_the_limit() {
        let mut entries = HashMap::new();
        for n in 0..10u64 {
            entries.insert(
                format!("q:\\made-up\\{n}.bin"),
                Entry {
                    size: n,
                    modified: 5,
                    used: 1000 + n,
                    present: 0b0010,
                    digests: [[0u8; 32], [n as u8; 32], [0u8; 32], [0u8; 32]],
                },
            );
        }
        let kept = decode(&encode(&entries, 4)).unwrap();
        let mut names: Vec<&str> = kept.keys().map(|k| k.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "q:\\made-up\\6.bin",
                "q:\\made-up\\7.bin",
                "q:\\made-up\\8.bin",
                "q:\\made-up\\9.bin"
            ]
        );
        assert_eq!(kept["q:\\made-up\\7.bin"], entries["q:\\made-up\\7.bin"]);
        assert_eq!(decode(&encode(&entries, 100)).unwrap(), entries);
    }
}
