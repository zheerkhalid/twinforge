//! Exact duplicates: files whose contents are identical byte for byte.
//!
//! The work is narrowed down in stages so that as little as possible is read:
//! 1. list the files;
//! 2. keep sizes that occur more than once (several names of one file on disk,
//!    i.e. hard links, count as a single file);
//! 3. hash the first and last block of each remaining file;
//! 4. hash the whole content of what still matches;
//! 5. on request, compare the matches byte by byte.
//!
//! Everything here is read-only; the only file written is the hash cache.

use crate::cache::{Cache, Kind};
use crate::hashing::{self, Digest, Side, EDGE_BLOCK, READ_BUFFER, WHOLE_LIMIT};
use crate::model::{FileEntry, Finder, Group, Item, Progress, ScanConfig, ScanResult};
use crate::sys::{self, Guard};
use crate::walk::{self, WalkOptions};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const MAX_ERRORS: usize = 200;

/// Finds groups of files with identical contents.
pub fn scan(cfg: &ScanConfig, progress: &Progress) -> ScanResult {
    scan_with(cfg, &Guard::real(), progress)
}

/// [`scan`] with an explicit guard (tests use their own).
pub fn scan_with(cfg: &ScanConfig, guard: &Guard, progress: &Progress) -> ScanResult {
    let started = Instant::now();
    let mut result = ScanResult::empty(Finder::Duplicates);
    progress.set_stage_count(if cfg.byte_verify { 5 } else { 4 });

    let tree = walk::walk_with(cfg, WalkOptions::ALL, guard, progress);
    result.files_scanned = tree.files.len() as u64;
    result.bytes_scanned = tree.bytes;
    let mut files = tree.files;
    let mut errors = tree.errors;
    errors.truncate(MAX_ERRORS);
    if progress.cancelled() {
        result.errors = errors;
        result.cancelled = true;
        result.elapsed_ms = started.elapsed().as_millis() as u64;
        return result;
    }
    // Path order everywhere: reads are roughly sequential on disk and the
    // outcome does not depend on thread timing.
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let run = Run {
        cfg,
        progress,
        files: &files,
        cache: cfg.cache_path.as_deref().map(Cache::open),
        threads: cfg.worker_count(),
        bytes_read: AtomicU64::new(0),
        cache_hits: AtomicU64::new(0),
        errors: Mutex::new(errors),
    };
    let mut found = run.find();
    found.sort_by_key(|f| f.members[0]);
    result.groups = found
        .iter()
        .map(|f| {
            let items = f
                .members
                .iter()
                .map(|&i| Item::new(files[i].clone()))
                .collect();
            Group::new(items, hashing::label(cfg.hash, &f.digest))
        })
        .collect();

    // A cancelled scan still keeps what it learned.
    if let Some(cache) = &run.cache {
        if let Err(e) = cache.save() {
            run.error(e);
        }
    }
    result.bytes_read = run.bytes_read.load(Ordering::Relaxed);
    result.cache_hits = run.cache_hits.load(Ordering::Relaxed);
    result.errors = run.errors.into_inner().unwrap_or_default();
    result.cancelled = progress.cancelled();
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    result
}

/// Files with the same content hash, as indices into the sorted file list
/// (ascending, so in path order).
struct Found {
    digest: Digest,
    members: Vec<usize>,
}

/// The outcome of comparing two files byte by byte.
enum Compared {
    Same,
    Differ,
    FirstUnreadable,
    SecondUnreadable,
    Cancelled,
}

/// Runs `work(state, n)` for every `n` in `0..count` on up to `threads`
/// threads. Items are handed out in ascending order; each thread gets its own
/// `state` (a read buffer). Stops handing out work once the scan is
/// cancelled. Entry `n` of the outcome is `None` when item `n` was not
/// finished or produced nothing.
fn in_parallel<S, T, I, W>(
    count: usize,
    threads: usize,
    progress: &Progress,
    init: I,
    work: W,
) -> Vec<Option<T>>
where
    T: Send,
    I: Fn() -> S + Sync,
    W: Fn(&mut S, usize) -> Option<T> + Sync,
{
    let mut out: Vec<Option<T>> = (0..count).map(|_| None).collect();
    if count == 0 {
        return out;
    }
    let next = AtomicUsize::new(0);
    let worker = || {
        let mut state = init();
        let mut done: Vec<(usize, T)> = Vec::new();
        while !progress.cancelled() {
            let n = next.fetch_add(1, Ordering::Relaxed);
            if n >= count {
                break;
            }
            if let Some(value) = work(&mut state, n) {
                done.push((n, value));
            }
        }
        done
    };
    let parts: Vec<Vec<(usize, T)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads.clamp(1, count))
            .map(|_| scope.spawn(worker))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (n, value) in parts.into_iter().flatten() {
        out[n] = Some(value);
    }
    out
}

/// The shared state of one scan.
struct Run<'a> {
    cfg: &'a ScanConfig,
    progress: &'a Progress,
    /// Sorted by path.
    files: &'a [FileEntry],
    cache: Option<Cache>,
    threads: usize,
    bytes_read: AtomicU64,
    cache_hits: AtomicU64,
    errors: Mutex<Vec<String>>,
}

impl Run<'_> {
    fn error(&self, text: String) {
        if let Ok(mut errors) = self.errors.lock() {
            if errors.len() < MAX_ERRORS {
                errors.push(text);
            }
        }
    }

    fn unreadable(&self, file: &FileEntry, e: &std::io::Error) {
        self.error(format!(
            "{}: could not be read and was skipped ({e}).",
            file.path.to_string_lossy()
        ));
    }

    fn changed(&self, file: &FileEntry) {
        self.error(format!(
            "{}: changed while it was being scanned and was skipped.",
            file.path.to_string_lossy()
        ));
    }

    fn read(&self, bytes: u64) {
        self.bytes_read.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Shows a path in the window now and then: every few files, and every large one.
    fn show(&self, n: usize, file: &FileEntry) {
        if n.is_multiple_of(16) || file.size >= READ_BUFFER as u64 {
            self.progress.set_current(&file.path);
        }
    }

    /// All stages after the listing.
    fn find(&self) -> Vec<Found> {
        let candidates = self.same_size();
        if self.progress.cancelled() {
            return Vec::new();
        }
        let (mut found, large) = self.edge_stage(candidates);
        if !self.progress.cancelled() {
            found.extend(self.full_stage(large));
        }
        if self.cfg.byte_verify {
            // Without the comparison nothing counts as confirmed.
            if self.progress.cancelled() {
                return Vec::new();
            }
            found = self.verify_stage(found);
        }
        found
    }

    /// Stage 2: the files whose size occurs at least twice, with the extra
    /// names of hard-linked files left out. Ascending indices.
    fn same_size(&self) -> Vec<usize> {
        let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, file) in self.files.iter().enumerate() {
            by_size.entry(file.size).or_default().push(i);
        }
        let mut candidates: Vec<usize> = by_size
            .into_values()
            .filter(|class| class.len() >= 2)
            .flatten()
            .collect();
        candidates.sort_unstable();
        self.progress
            .set_stage("Comparing sizes", candidates.len() as u64, 0);

        // Asking for the identity opens the file, so only candidates are asked.
        let ids = in_parallel(
            candidates.len(),
            self.threads,
            self.progress,
            || (),
            |_, n| {
                let id = sys::file_id(&self.files[candidates[n]].path);
                self.progress.add_item();
                id
            },
        );
        if self.progress.cancelled() {
            return Vec::new();
        }
        // The first name in path order stands for the file; a file whose
        // identity cannot be read counts as a file of its own.
        let mut seen: HashSet<(u32, u64)> = HashSet::new();
        let mut classes: HashMap<u64, Vec<usize>> = HashMap::new();
        for (n, &i) in candidates.iter().enumerate() {
            if let Some(id) = ids[n] {
                if id.links > 1 && !seen.insert((id.volume, id.index)) {
                    continue;
                }
            }
            classes.entry(self.files[i].size).or_default().push(i);
        }
        let mut kept: Vec<usize> = classes
            .into_values()
            .filter(|class| class.len() >= 2)
            .flatten()
            .collect();
        kept.sort_unstable();
        kept
    }

    /// Groups files by size and digest and keeps the groups of two or more.
    fn group(&self, candidates: &[usize], digests: Vec<Option<Digest>>) -> Vec<Found> {
        let mut map: HashMap<(u64, Digest), Vec<usize>> = HashMap::new();
        for (&i, digest) in candidates.iter().zip(digests) {
            if let Some(digest) = digest {
                map.entry((self.files[i].size, digest)).or_default().push(i);
            }
        }
        let mut found: Vec<Found> = map
            .into_iter()
            .filter(|(_, members)| members.len() >= 2)
            .map(|((_, digest), members)| Found { digest, members })
            .collect();
        found.sort_by_key(|f| f.members[0]);
        found
    }

    /// Stage 3: hash the first and last block. Small files are hashed whole
    /// here, so their groups are final; of the larger files the ones that
    /// still match are returned for stage 4.
    fn edge_stage(&self, candidates: Vec<usize>) -> (Vec<Found>, Vec<usize>) {
        let bytes: u64 = candidates
            .iter()
            .map(|&i| self.files[i].size.min(WHOLE_LIMIT))
            .sum();
        self.progress.set_stage(
            "Reading the first and last blocks",
            candidates.len() as u64,
            bytes,
        );
        let digests = in_parallel(
            candidates.len(),
            self.threads,
            self.progress,
            || vec![0u8; WHOLE_LIMIT as usize],
            |buf, n| {
                let file = &self.files[candidates[n]];
                self.show(n, file);
                let kind = if file.size <= WHOLE_LIMIT {
                    Kind::Full
                } else {
                    Kind::Partial
                };
                let digest = self.digest(file, kind, buf);
                self.progress.add_item();
                digest
            },
        );
        let mut done = Vec::new();
        let mut large = Vec::new();
        for found in self.group(&candidates, digests) {
            if self.files[found.members[0]].size <= WHOLE_LIMIT {
                done.push(found);
            } else {
                large.extend(found.members);
            }
        }
        large.sort_unstable();
        (done, large)
    }

    /// Stage 4: hash the whole content of the remaining files.
    fn full_stage(&self, candidates: Vec<usize>) -> Vec<Found> {
        let bytes: u64 = candidates.iter().map(|&i| self.files[i].size).sum();
        self.progress
            .set_stage("Reading whole files", candidates.len() as u64, bytes);
        let digests = in_parallel(
            candidates.len(),
            self.threads,
            self.progress,
            || vec![0u8; READ_BUFFER],
            |buf, n| {
                let file = &self.files[candidates[n]];
                self.show(n, file);
                let digest = self.digest(file, Kind::Full, buf);
                self.progress.add_item();
                digest
            },
        );
        self.group(&candidates, digests)
    }

    /// One hash of one file: from the cache when the file is exactly as it
    /// was when the hash was recorded, from the disk otherwise. `None` when
    /// the file was skipped (an error is noted) or the scan was cancelled.
    fn digest(&self, entry: &FileEntry, kind: Kind, buf: &mut [u8]) -> Option<Digest> {
        let algo = self.cfg.hash;
        let planned = match kind {
            Kind::Partial => WHOLE_LIMIT,
            Kind::Full => entry.size,
        };
        let mut file = match hashing::open_shared(&entry.path) {
            Ok(f) => f,
            Err(e) => {
                self.unreadable(entry, &e);
                self.progress.add_bytes(planned);
                return None;
            }
        };
        let before = match hashing::stamp(&file) {
            Ok(s) => s,
            Err(e) => {
                self.unreadable(entry, &e);
                self.progress.add_bytes(planned);
                return None;
            }
        };
        if before.size != entry.size {
            self.changed(entry);
            self.progress.add_bytes(planned);
            return None;
        }
        if let Some(cache) = &self.cache {
            if let Some(digest) = cache.get(&entry.path, before, algo, kind) {
                self.cache_hits.fetch_add(1, Ordering::Relaxed);
                self.progress.add_bytes(planned);
                return Some(digest);
            }
        }

        let mut counted = 0u64;
        let outcome = match kind {
            Kind::Partial => hashing::hash_edges(&mut file, entry.size, algo, buf).map(|digest| {
                counted = 2 * EDGE_BLOCK as u64;
                self.read(counted);
                self.progress.add_bytes(counted);
                Some((digest, entry.size))
            }),
            Kind::Full => hashing::hash_stream(&mut file, algo, buf, |n| {
                counted += n;
                self.read(n);
                self.progress.add_bytes(n);
                !self.progress.cancelled()
            }),
        };
        let (digest, total) = match outcome {
            Ok(Some(pair)) => pair,
            Ok(None) => return None,
            Err(e) => {
                self.unreadable(entry, &e);
                self.progress.add_bytes(planned.saturating_sub(counted));
                return None;
            }
        };
        self.progress.add_bytes(planned.saturating_sub(counted));
        // The hash is only worth something if the file stood still meanwhile.
        if total != entry.size || hashing::stamp(&file).ok() != Some(before) {
            self.changed(entry);
            return None;
        }
        if let Some(cache) = &self.cache {
            cache.put(&entry.path, before, algo, kind, digest);
        }
        Some(digest)
    }

    /// Stage 5: inside each group every file is compared with the first one;
    /// files that differ are split off (and compared among themselves).
    fn verify_stage(&self, found: Vec<Found>) -> Vec<Found> {
        let others = |f: &Found| f.members.len() as u64 - 1;
        let items: u64 = found.iter().map(others).sum();
        let bytes: u64 = found
            .iter()
            .map(|f| others(f) * self.files[f.members[0]].size)
            .sum();
        self.progress
            .set_stage("Comparing byte by byte", items, bytes);
        let checked = in_parallel(
            found.len(),
            self.threads,
            self.progress,
            || (vec![0u8; READ_BUFFER], vec![0u8; READ_BUFFER]),
            |bufs, n| self.verify_group(&found[n].members, bufs),
        );
        let mut out = Vec::new();
        for (group, parts) in found.iter().zip(checked) {
            for members in parts.unwrap_or_default() {
                out.push(Found {
                    digest: group.digest,
                    members,
                });
            }
        }
        out
    }

    /// Splits one hash group into sets of files that are truly identical.
    /// `None` when the scan was cancelled before the group was finished.
    fn verify_group(
        &self,
        members: &[usize],
        bufs: &mut (Vec<u8>, Vec<u8>),
    ) -> Option<Vec<Vec<usize>>> {
        let mut confirmed = Vec::new();
        let mut rest = members.to_vec();
        while rest.len() >= 2 {
            let base = rest[0];
            let mut same = vec![base];
            let mut differ = Vec::new();
            let mut base_lost = false;
            for &other in &rest[1..] {
                if base_lost {
                    differ.push(other);
                    continue;
                }
                match self.compare(base, other, bufs) {
                    Compared::Cancelled => return None,
                    Compared::Same => same.push(other),
                    Compared::Differ => differ.push(other),
                    Compared::SecondUnreadable => {}
                    Compared::FirstUnreadable => {
                        base_lost = true;
                        differ.push(other);
                    }
                }
            }
            if base_lost {
                // Start over without the lost file.
                same.remove(0);
                same.extend(differ);
                same.sort_unstable();
                rest = same;
            } else {
                if same.len() >= 2 {
                    confirmed.push(same);
                }
                rest = differ;
            }
        }
        Some(confirmed)
    }

    fn compare(&self, first: usize, second: usize, bufs: &mut (Vec<u8>, Vec<u8>)) -> Compared {
        let (a, b) = (&self.files[first], &self.files[second]);
        self.progress.set_current(&b.path);
        let size = b.size;
        let open = |entry: &FileEntry| -> Option<std::fs::File> {
            let file = match hashing::open_shared(&entry.path) {
                Ok(f) => f,
                Err(e) => {
                    self.unreadable(entry, &e);
                    return None;
                }
            };
            match hashing::stamp(&file) {
                Ok(s) if s.size == entry.size => Some(file),
                Ok(_) => {
                    self.changed(entry);
                    None
                }
                Err(e) => {
                    self.unreadable(entry, &e);
                    None
                }
            }
        };
        let skipped = |outcome: Compared| {
            self.progress.add_item();
            self.progress.add_bytes(size);
            outcome
        };
        let Some(mut file_a) = open(a) else {
            return skipped(Compared::FirstUnreadable);
        };
        let Some(mut file_b) = open(b) else {
            return skipped(Compared::SecondUnreadable);
        };
        let mut counted = 0u64;
        let outcome =
            hashing::same_content(&mut file_a, &mut file_b, &mut bufs.0, &mut bufs.1, |n| {
                counted += n;
                self.read(2 * n);
                self.progress.add_bytes(n);
                !self.progress.cancelled()
            });
        self.progress.add_item();
        self.progress.add_bytes(size.saturating_sub(counted));
        match outcome {
            Ok(None) => Compared::Cancelled,
            Ok(Some(true)) => Compared::Same,
            Ok(Some(false)) => Compared::Differ,
            Err((Side::First, e)) => {
                self.unreadable(a, &e);
                Compared::FirstUnreadable
            }
            Err((Side::Second, e)) => {
                self.unreadable(b, &e);
                Compared::SecondUnreadable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashing::testkit::{age, filler, put, put_aged, shape, temp_dir};
    use crate::model::HashAlgo;
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};

    fn run(cfg: &ScanConfig) -> ScanResult {
        scan_with(cfg, &Guard::none(), &Progress::new())
    }

    fn names(groups: &[&[&str]]) -> Vec<Vec<String>> {
        groups
            .iter()
            .map(|g| g.iter().map(|n| n.to_string()).collect())
            .collect()
    }

    /// Two large twins, a same-size stranger, a near-twin that only differs
    /// in the middle, three small triplets and a small stranger.
    fn plant(dir: &Path) {
        let big = filler(1, 100_000);
        let mut near = big.clone();
        near[50_000] ^= 0xFF;
        put(&dir.join("one\\a.bin"), &big);
        put(&dir.join("two\\b.bin"), &big);
        put(&dir.join("two\\stranger.bin"), &filler(2, 100_000));
        put(&dir.join("one\\near.bin"), &near);
        let small = filler(3, 1_000);
        put(&dir.join("s1.dat"), &small);
        put(&dir.join("one\\s2.dat"), &small);
        put(&dir.join("two\\deep\\s3.dat"), &small);
        put(&dir.join("s4.dat"), &filler(4, 1_000));
        put(&dir.join("lonely.dat"), &filler(5, 777));
    }

    #[test]
    fn groups_identical_files_only() {
        let dir = temp_dir("dupes-basic");
        plant(&dir);
        let expected = names(&[&["a.bin", "b.bin"], &["s1.dat", "s2.dat", "s3.dat"]]);
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let progress = Progress::new();
        let result = scan_with(&cfg, &Guard::none(), &progress);
        assert_eq!(shape(&result), expected);
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(!result.cancelled);
        assert_eq!(result.files_scanned, 9);
        assert_eq!(result.bytes_scanned, 400_000 + 4_000 + 777);
        assert_eq!(result.cache_hits, 0);
        // Edges of the four large files, eight small files... no: the lonely
        // size is never opened. Edges 4 x 32 KiB, four small files whole,
        // then the three large files that share their edges read whole.
        assert_eq!(result.bytes_read, 4 * 32_768 + 4 * 1_000 + 3 * 100_000);
        let snap = progress.snapshot();
        assert_eq!((snap.stage_no, snap.stage_count), (4, 4));
        assert_eq!(snap.stage, "Reading whole files");
        assert_eq!((snap.items_done, snap.items_total), (3, 3));
        assert_eq!((snap.bytes_done, snap.bytes_total), (300_000, 300_000));
        for group in &result.groups {
            assert!(group.label.starts_with("BLAKE3 "), "{}", group.label);
            assert_eq!(group.label.chars().count(), "BLAKE3 ".len() + 13);
            assert!(group
                .items
                .windows(2)
                .all(|w| w[0].file.path < w[1].file.path));
            assert!(group
                .items
                .iter()
                .all(|i| i.detail == crate::model::Detail::None && !i.file.is_dir));
        }
        let big = result.groups.iter().find(|g| g.items.len() == 2).unwrap();
        let expected_label = hashing::label(
            HashAlgo::Blake3,
            &hashing::hash_bytes(HashAlgo::Blake3, &filler(1, 100_000)),
        );
        assert_eq!(big.label, expected_label);

        // The other algorithm and other thread counts give the same groups.
        cfg.hash = HashAlgo::Xxh3;
        let xx = run(&cfg);
        assert_eq!(shape(&xx), expected);
        assert!(xx.groups.iter().all(|g| g.label.starts_with("XXH3 ")));
        for threads in [1, 2, 8] {
            cfg.threads = threads;
            let again = run(&cfg);
            assert_eq!(again.groups, xx.groups, "{threads} threads");
            assert_eq!(again.bytes_read, xx.bytes_read);
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sizes_around_the_whole_file_limit() {
        let dir = temp_dir("dupes-limit");
        for (n, size) in [32_767usize, 32_768, 32_769, 40_000]
            .into_iter()
            .enumerate()
        {
            let data = filler(10 + n as u64, size);
            let mut other = data.clone();
            other[size / 2] ^= 1;
            put(&dir.join(format!("x{n}.bin")), &data);
            put(&dir.join(format!("y{n}.bin")), &data);
            put(&dir.join(format!("z{n}.bin")), &other);
        }
        let result = run(&ScanConfig::for_paths(&[&dir]));
        assert_eq!(
            shape(&result),
            names(&[
                &["x0.bin", "y0.bin"],
                &["x1.bin", "y1.bin"],
                &["x2.bin", "y2.bin"],
                &["x3.bin", "y3.bin"],
            ])
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hard_links_count_as_one_file() {
        let dir = temp_dir("dupes-links");
        let data = filler(6, 80_000);
        put(&dir.join("a\\original.bin"), &data);
        fs::hard_link(dir.join("a\\original.bin"), dir.join("a\\second-name.bin")).unwrap();
        put(&dir.join("b\\copy.bin"), &data);
        // Two names of one file and nothing else: no duplicate at all.
        put(&dir.join("c\\solo.bin"), &filler(7, 5_000));
        fs::hard_link(dir.join("c\\solo.bin"), dir.join("c\\solo-again.bin")).unwrap();
        let result = run(&ScanConfig::for_paths(&[&dir]));
        assert_eq!(shape(&result), names(&[&["copy.bin", "original.bin"]]));
        assert_eq!(result.groups[0].items.len(), 2);
        assert_eq!(result.files_scanned, 5);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn second_run_uses_the_cache() {
        let dir = temp_dir("dupes-cache");
        let data = dir.join("data");
        let cache = dir.join("app\\hashes.bin");
        let big = filler(1, 500_000);
        let mut near = big.clone();
        near[250_000] ^= 1;
        for name in ["a.bin", "sub\\b.bin", "sub\\c.bin"] {
            put_aged(&data.join(name), &big, 3_600);
        }
        put_aged(&data.join("near.bin"), &near, 3_600);
        put_aged(&data.join("s1.txt"), b"small twin", 3_600);
        put_aged(&data.join("s2.txt"), b"small twin", 3_600);
        let expected = names(&[&["a.bin", "b.bin", "c.bin"], &["s1.txt", "s2.txt"]]);

        let mut cfg = ScanConfig::for_paths(&[&data]);
        cfg.cache_path = Some(cache.clone());
        let first = run(&cfg);
        assert_eq!(shape(&first), expected);
        assert_eq!(first.cache_hits, 0);
        assert_eq!(first.bytes_read, 4 * 32_768 + 2 * 10 + 4 * 500_000);
        assert!(cache.exists());

        let second = run(&cfg);
        assert_eq!(second.groups, first.groups);
        // Four partial, four full and two small-file hashes.
        assert_eq!(second.cache_hits, 10);
        assert_eq!(second.bytes_read, 0);

        // The other algorithm has its own entries.
        cfg.hash = HashAlgo::Xxh3;
        let other = run(&cfg);
        assert_eq!(shape(&other), expected);
        assert_eq!(other.cache_hits, 0);
        assert_eq!(run(&cfg).cache_hits, 10);
        cfg.hash = HashAlgo::Blake3;
        assert_eq!(run(&cfg).cache_hits, 10, "both sets of entries are kept");

        // New content with a new size: no longer a twin.
        put_aged(&data.join("sub\\c.bin"), &filler(2, 400_000), 1_800);
        let third = run(&cfg);
        assert_eq!(
            shape(&third),
            names(&[&["a.bin", "b.bin"], &["s1.txt", "s2.txt"]])
        );

        // New content with the same size and a different time: read again.
        let mut edited = big.clone();
        edited[250_001] ^= 1;
        put_aged(&data.join("sub\\b.bin"), &edited, 900);
        let fourth = run(&cfg);
        assert_eq!(shape(&fourth), names(&[&["s1.txt", "s2.txt"]]));
        assert_eq!(fourth.bytes_read, 32_768 + 500_000, "only b.bin was read");
        assert_eq!(fourth.cache_hits, 2 + 2 + 2);

        // The same content with only a new time is read again and still matches.
        put_aged(&data.join("sub\\b.bin"), &big, 600);
        let fifth = run(&cfg);
        assert_eq!(
            shape(&fifth),
            names(&[&["a.bin", "b.bin"], &["s1.txt", "s2.txt"]])
        );
        assert_eq!(fifth.bytes_read, 32_768 + 500_000);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_time_stamp_alone_invalidates() {
        let dir = temp_dir("dupes-touch");
        let data = dir.join("data");
        let twin = filler(8, 60_000);
        put_aged(&data.join("a.bin"), &twin, 3_600);
        put_aged(&data.join("b.bin"), &twin, 3_600);
        let mut cfg = ScanConfig::for_paths(&[&data]);
        cfg.cache_path = Some(dir.join("hashes.bin"));
        assert_eq!(run(&cfg).bytes_read, 2 * (32_768 + 60_000));
        assert_eq!(run(&cfg).bytes_read, 0);
        age(&data.join("b.bin"), 1_200);
        let touched = run(&cfg);
        assert_eq!(touched.bytes_read, 32_768 + 60_000);
        assert_eq!(touched.cache_hits, 2);
        assert_eq!(shape(&touched), names(&[&["a.bin", "b.bin"]]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_cache_is_ignored_and_none_writes_nothing() {
        let dir = temp_dir("dupes-badcache");
        let data = dir.join("data");
        put_aged(&data.join("a.bin"), &filler(1, 70_000), 3_600);
        put_aged(&data.join("b.bin"), &filler(1, 70_000), 3_600);
        let expected = names(&[&["a.bin", "b.bin"]]);
        let count = |d: &Path| fs::read_dir(d).unwrap().count();

        // No cache path: nothing appears anywhere in the temp tree.
        let mut cfg = ScanConfig::for_paths(&[&data]);
        let plain = run(&cfg);
        assert_eq!(shape(&plain), expected);
        assert_eq!((count(&dir), count(&data)), (1, 2));
        assert_eq!(run(&cfg).cache_hits, 0);

        let cache = dir.join("hashes.bin");
        cfg.cache_path = Some(cache.clone());
        let noise = filler(3, 5_000);
        for rubbish in [&b""[..], &b"garbage"[..], &noise[..]] {
            fs::write(&cache, rubbish).unwrap();
            let result = run(&cfg);
            assert_eq!(shape(&result), expected);
            assert_eq!(result.cache_hits, 0);
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(run(&cfg).cache_hits, 4, "the cache was rebuilt");
        }
        // A truncated cache.
        let good = fs::read(&cache).unwrap();
        fs::write(&cache, &good[..good.len() - 5]).unwrap();
        let result = run(&cfg);
        assert_eq!(shape(&result), expected);
        assert_eq!(result.cache_hits, 0);
        assert_eq!(count(&dir), 2, "only the data folder and the cache file");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_scans_return_quickly() {
        let dir = temp_dir("dupes-cancel");
        plant(&dir);
        let mut cfg = ScanConfig::for_paths(&[&dir.join("one"), &dir.join("two")]);
        cfg.cache_path = Some(dir.join("hashes.bin"));
        let progress = Progress::new();
        progress.cancel();
        let started = Instant::now();
        let result = scan_with(&cfg, &Guard::none(), &progress);
        assert!(result.cancelled);
        assert!(result.groups.is_empty());
        assert_eq!(result.bytes_read, 0);
        assert!(started.elapsed().as_secs() < 5);
        assert!(!dir.join("hashes.bin").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelling_between_stages_keeps_only_confirmed_groups() {
        let dir = temp_dir("dupes-cancel-late");
        plant(&dir);
        let cfg = ScanConfig::for_paths(&[&dir]);
        let tree = walk::walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        let mut files = tree.files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let progress = Progress::new();
        let run = Run {
            cfg: &cfg,
            progress: &progress,
            files: &files,
            cache: None,
            threads: 2,
            bytes_read: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            errors: Mutex::new(Vec::new()),
        };
        let candidates = run.same_size();
        assert_eq!(candidates.len(), 8);
        let (small, large) = run.edge_stage(candidates);
        assert_eq!((small.len(), large.len()), (1, 3));
        progress.cancel();
        // Nothing is read after the cancel, so the large files stay unconfirmed.
        let before = run.bytes_read.load(Ordering::Relaxed);
        assert!(run.full_stage(large).is_empty());
        assert_eq!(run.bytes_read.load(Ordering::Relaxed), before);
        assert!(run.verify_stage(small).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn byte_verify_confirms_and_splits() {
        let dir = temp_dir("dupes-verify");
        plant(&dir);
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.byte_verify = true;
        let progress = Progress::new();
        let result = scan_with(&cfg, &Guard::none(), &progress);
        assert_eq!(
            shape(&result),
            names(&[&["a.bin", "b.bin"], &["s1.dat", "s2.dat", "s3.dat"]])
        );
        // The hashing stages plus one comparison of the large pair and two of the small files.
        let hashing_bytes = 4 * 32_768 + 4 * 1_000 + 3 * 100_000;
        assert_eq!(result.bytes_read, hashing_bytes + 2 * 100_000 + 4 * 1_000);
        let snap = progress.snapshot();
        assert_eq!((snap.stage_no, snap.stage_count), (5, 5));
        assert_eq!(snap.stage, "Comparing byte by byte");
        assert_eq!((snap.items_done, snap.items_total), (3, 3));
        assert_eq!((snap.bytes_done, snap.bytes_total), (102_000, 102_000));

        // A forged hash group (as a hash collision or a poisoned cache would
        // produce) is taken apart by the comparison.
        let tree = walk::walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        let mut files = tree.files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let index = |name: &str| {
            files
                .iter()
                .position(|f| crate::util::file_name(&f.path) == name)
                .unwrap()
        };
        let progress = Progress::new();
        let run = Run {
            cfg: &cfg,
            progress: &progress,
            files: &files,
            cache: None,
            threads: 4,
            bytes_read: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            errors: Mutex::new(Vec::new()),
        };
        let mut members = vec![
            index("near.bin"),
            index("a.bin"),
            index("stranger.bin"),
            index("b.bin"),
        ];
        members.sort_unstable();
        let forged = vec![
            Found {
                digest: [7u8; 32],
                members,
            },
            Found {
                digest: [9u8; 32],
                members: vec![index("s1.dat"), index("s4.dat")],
            },
        ];
        let checked = run.verify_stage(forged);
        assert_eq!(checked.len(), 1);
        let mut pair = vec![index("a.bin"), index("b.bin")];
        pair.sort_unstable();
        assert_eq!(checked[0].members, pair);
        assert_eq!(checked[0].digest, [7u8; 32]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filters_are_honoured() {
        let dir = temp_dir("dupes-filters");
        plant(&dir);
        put(&dir.join("empty-1.dat"), b"");
        put(&dir.join("empty-2.dat"), b"");
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.min_size = 5_000;
        assert_eq!(shape(&run(&cfg)), names(&[&["a.bin", "b.bin"]]));
        cfg.min_size = 1;
        cfg.only_extensions = vec!["dat".into()];
        assert_eq!(shape(&run(&cfg)), names(&[&["s1.dat", "s2.dat", "s3.dat"]]));
        cfg.only_extensions.clear();
        cfg.skip_extensions = vec!["dat".into()];
        assert_eq!(shape(&run(&cfg)), names(&[&["a.bin", "b.bin"]]));
        cfg.skip_extensions.clear();
        cfg.max_size = Some(2_000);
        assert_eq!(shape(&run(&cfg)), names(&[&["s1.dat", "s2.dat", "s3.dat"]]));
        cfg.max_size = None;
        cfg.exclude = vec![dir.join("two")];
        assert_eq!(shape(&run(&cfg)), names(&[&["s1.dat", "s2.dat"]]));
        cfg.exclude.clear();
        cfg.recursive = false;
        assert!(run(&cfg).groups.is_empty());
        // Empty files only take part when the smallest size is zero.
        cfg.recursive = true;
        cfg.min_size = 0;
        cfg.max_size = Some(0);
        assert_eq!(shape(&run(&cfg)), names(&[&["empty-1.dat", "empty-2.dat"]]));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn junctions_are_not_followed() {
        let dir = temp_dir("dupes-junction");
        let outside = temp_dir("dupes-junction-outside");
        let data = filler(1, 50_000);
        put(&dir.join("real\\a.bin"), &data);
        put(&dir.join("real\\b.bin"), &data);
        put(&outside.join("c.bin"), &data);
        assert!(sys::make_junction(&dir.join("link"), &outside));
        assert!(dir.join("link\\c.bin").exists(), "the junction works");
        let result = run(&ScanConfig::for_paths(&[&dir]));
        assert_eq!(shape(&result), names(&[&["a.bin", "b.bin"]]));
        assert_eq!(result.files_scanned, 2);
        fs::remove_dir(dir.join("link")).unwrap();
        assert!(outside.join("c.bin").exists());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn unreadable_files_are_skipped_with_a_note() {
        let dir = temp_dir("dupes-locked");
        let data = filler(1, 90_000);
        for name in ["a.bin", "b.bin", "locked.bin"] {
            put(&dir.join(name), &data);
        }
        // Another program holds the file without sharing it.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.join("locked.bin"))
            .unwrap();
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        for verify in [false, true] {
            cfg.byte_verify = verify;
            let result = run(&cfg);
            assert_eq!(shape(&result), names(&[&["a.bin", "b.bin"]]));
            assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
            assert!(result.errors[0].contains("locked.bin"));
            assert!(result.errors[0].ends_with('.'));
        }
        drop(held);
        assert_eq!(run(&cfg).groups[0].items.len(), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_that_changed_since_the_listing_are_skipped() {
        let dir = temp_dir("dupes-changed");
        let data = filler(1, 90_000);
        for name in ["a.bin", "b.bin", "c.bin"] {
            put(&dir.join(name), &data);
        }
        let cfg = ScanConfig::for_paths(&[&dir]);
        let tree = walk::walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        let mut files = tree.files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        // After the listing one file shrinks.
        fs::write(dir.join("c.bin"), &data[..10_000]).unwrap();
        let progress = Progress::new();
        let run = Run {
            cfg: &cfg,
            progress: &progress,
            files: &files,
            cache: None,
            threads: 3,
            bytes_read: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            errors: Mutex::new(Vec::new()),
        };
        let found = run.find();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].members, [0, 1]);
        let errors = run.errors.into_inner().unwrap();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("c.bin") && errors[0].contains("changed"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parallel_helper_keeps_order_and_stops_on_cancel() {
        let progress = Progress::new();
        let squares = in_parallel(
            1_000,
            8,
            &progress,
            || 0u64,
            |calls, n| {
                *calls += 1;
                (n % 7 != 0).then_some(n * n)
            },
        );
        assert_eq!(squares.len(), 1_000);
        for (n, value) in squares.iter().enumerate() {
            assert_eq!(*value, (n % 7 != 0).then_some(n * n));
        }
        assert!(in_parallel(0, 8, &progress, || (), |_, n| Some(n)).is_empty());
        progress.cancel();
        let none = in_parallel(50, 4, &progress, || (), |_, n| Some(n));
        assert!(none.iter().all(|v| v.is_none()));
    }

    /// Not part of the normal run: measures throughput on about 200 MB of
    /// generated duplicates. Run with
    /// `cargo test --lib dupes::tests::speed -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn speed() {
        let dir = temp_dir("dupes-speed");
        let data: PathBuf = dir.join("data");
        let mut total = 0u64;
        for set in 0..4u64 {
            let blob = filler(100 + set, 25 * 1024 * 1024 + set as usize * 4096);
            for copy in 0..2 {
                put(&data.join(format!("set-{set}\\copy-{copy}.bin")), &blob);
                total += blob.len() as u64;
            }
        }
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        // Let the fresh time stamps settle so that the cache accepts them.
        std::thread::sleep(std::time::Duration::from_millis(200));
        for algo in [HashAlgo::Blake3, HashAlgo::Xxh3] {
            let mut cfg = ScanConfig::for_paths(&[&data]);
            cfg.hash = algo;
            cfg.cache_path = Some(dir.join(format!("hashes-{algo:?}.bin")));
            let started = Instant::now();
            let first = run(&cfg);
            let first_secs = started.elapsed().as_secs_f64();
            let started = Instant::now();
            let second = run(&cfg);
            let second_secs = started.elapsed().as_secs_f64();
            let started = Instant::now();
            let third = run(&cfg);
            let third_secs = started.elapsed().as_secs_f64();
            assert_eq!(third.groups, first.groups);
            assert_eq!(first.groups.len(), 4);
            assert_eq!(second.groups, first.groups);
            assert_eq!(second.bytes_read, 0);
            println!(
                "{algo:?}, {} threads: first scan read {:.0} MB of {:.0} MB in {:.2} s = {:.0} MB/s; \
                 cached scan {:.3} s, again {:.3} s ({} cache hits, {} bytes read)",
                cfg.worker_count(),
                mb(first.bytes_read),
                mb(total),
                first_secs,
                mb(first.bytes_read) / first_secs,
                second_secs,
                third_secs,
                second.cache_hits,
                second.bytes_read
            );
            cfg.threads = 1;
            cfg.cache_path = None;
            let started = Instant::now();
            let single = run(&cfg);
            let secs = started.elapsed().as_secs_f64();
            println!(
                "{algo:?}, 1 thread, no cache: {:.0} MB in {:.2} s = {:.0} MB/s",
                mb(single.bytes_read),
                secs,
                mb(single.bytes_read) / secs
            );
            cfg.threads = 0;
            cfg.byte_verify = true;
            let started = Instant::now();
            let verified = run(&cfg);
            let secs = started.elapsed().as_secs_f64();
            assert_eq!(verified.groups, first.groups);
            println!(
                "{algo:?} with byte-by-byte check, no cache: {:.0} MB in {:.2} s = {:.0} MB/s",
                mb(verified.bytes_read),
                secs,
                mb(verified.bytes_read) / secs
            );
        }
        fs::remove_dir_all(&dir).unwrap();
        assert!(!dir.exists());
    }
}
