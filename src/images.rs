//! Similar pictures: reads every picture, takes its perceptual signature
//! (see [`crate::phash`]) and groups pictures whose signatures are close.
//! Also decodes thumbnails for the window and words the "how do these two
//! differ" hint.

use crate::model::{
    Detail, FileEntry, Finder, Group, Item, Progress, ScanConfig, ScanResult, Thumb,
};
use crate::phash::{self, Signature};
use crate::sys::Guard;
use crate::util::{extension, norm_path};
use crate::walk::{self, WalkOptions};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

/// File types that are read as pictures.
pub const EXTENSIONS: [&str; 10] = [
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "bmp", "webp", "tif", "tiff",
];

const MAX_ERRORS: usize = 200;

pub fn is_picture(path: &Path) -> bool {
    EXTENSIONS.contains(&extension(path).as_str()) || crate::wic::handles(path)
}

/// How large the pictures decoded by Windows are when they are fingerprinted.
const WINDOWS_HASH_SIDE: u32 = 512;

/// Decodes a picture for fingerprinting and reports its real size in pixels.
/// HEIC, AVIF and camera RAW files go to the Windows codecs (see
/// [`crate::wic`]); a picture in a variant the built-in decoders do not
/// support gets a second chance there as well. A damaged file does not:
/// Windows would hand back whatever part of it can still be read, and half a
/// picture must never be matched as if it were whole.
pub fn decode_sized(path: &Path) -> Result<(image::DynamicImage, u32, u32), String> {
    let by_windows = |path: &Path| {
        crate::wic::decode(path, WINDOWS_HASH_SIDE).map(|d| {
            (
                image::DynamicImage::ImageRgba8(d.image),
                d.full_width,
                d.full_height,
            )
        })
    };
    if crate::wic::handles(path) {
        return by_windows(path);
    }
    match decode_builtin(path) {
        Ok(img) => {
            let (w, h) = (img.width(), img.height());
            Ok((img, w, h))
        }
        Err(e) if unsupported(&e) => by_windows(path).map_err(|_| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// The picture is fine but uses something the built-in decoders cannot do.
fn unsupported(e: &image::ImageError) -> bool {
    matches!(e, image::ImageError::Unsupported(_))
}

/// Decodes a picture with limits that keep a hostile file from eating all
/// memory, and turns it upright according to its EXIF orientation.
pub fn decode(path: &Path) -> Result<image::DynamicImage, String> {
    decode_builtin(path).map_err(|e| e.to_string())
}

fn decode_builtin(path: &Path) -> Result<image::DynamicImage, image::ImageError> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    limits.max_image_width = Some(40_000);
    limits.max_image_height = Some(40_000);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    use image::ImageDecoder;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = image::DynamicImage::from_decoder(decoder)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Decodes a picture and scales it to fit `max_side` x `max_side`.
pub fn load_thumbnail(path: &Path, max_side: u32) -> Option<Thumb> {
    let max_side = max_side.clamp(8, 4096);
    let by_windows = |path: &Path| {
        let d = crate::wic::decode(path, max_side).ok()?;
        Some(Thumb {
            width: d.image.width(),
            height: d.image.height(),
            rgba: d.image.into_raw(),
            full_width: d.full_width,
            full_height: d.full_height,
        })
    };
    if crate::wic::handles(path) {
        return by_windows(path);
    }
    let img = match std::panic::catch_unwind(|| decode_builtin(path)).ok()? {
        Ok(img) => img,
        Err(e) if unsupported(&e) => return by_windows(path),
        Err(_) => return None,
    };
    let (full_width, full_height) = (img.width(), img.height());
    if full_width == 0 || full_height == 0 {
        return None;
    }
    let small = if full_width <= max_side && full_height <= max_side {
        img.to_rgba8()
    } else {
        img.thumbnail(max_side, max_side).to_rgba8()
    };
    Some(Thumb {
        width: small.width(),
        height: small.height(),
        rgba: small.into_raw(),
        full_width,
        full_height,
    })
}

/// A plain-language hint about how two pictures of a group differ.
pub fn difference_hint(a: &Item, b: &Item) -> String {
    let dims = |i: &Item| match i.detail {
        Detail::Image {
            width,
            height,
            similarity,
        } => Some((width, height, similarity)),
        _ => None,
    };
    let (Some((aw, ah, a_sim)), Some((bw, bh, b_sim))) = (dims(a), dims(b)) else {
        return "No picture details are known for one of these files.".into();
    };
    let (ext_a, ext_b) = (
        extension(&a.file.path).to_uppercase(),
        extension(&b.file.path).to_uppercase(),
    );
    let kind = |e: &str| match e {
        "JPG" | "JPEG" | "JPE" | "JFIF" => "JPEG".to_string(),
        "TIF" | "TIFF" => "TIFF".to_string(),
        other => other.to_string(),
    };
    let (kind_a, kind_b) = (kind(&ext_a), kind(&ext_b));
    let alike = a_sim.min(b_sim);
    if (aw, ah) == (bw, bh) {
        if a.file.size == b.file.size && kind_a == kind_b {
            return "Identical in resolution and file size.".into();
        }
        if kind_a != kind_b {
            return format!("Different format ({kind_a} and {kind_b}), same size in pixels.");
        }
        if alike < 97 {
            return format!(
                "Same size in pixels, looks {alike}% alike: check for edits before removing."
            );
        }
        return if a.file.size < b.file.size {
            "Same size in pixels but a smaller file: more strongly compressed.".into()
        } else {
            "Same size in pixels but a larger file: less strongly compressed.".into()
        };
    }
    // Same shape (also when turned on its side)?
    let ratio = |w: u32, h: u32| w.max(1) as f64 / h.max(1) as f64;
    let (ra, rb) = (ratio(aw, ah), ratio(bw, bh));
    let same_shape = (ra / rb - 1.0).abs() < 0.03;
    let turned = (ra * rb - 1.0).abs() < 0.03 && !same_shape;
    if turned {
        return format!("Turned on its side ({aw} x {ah} and {bw} x {bh}).");
    }
    if !same_shape {
        return format!("Different shape ({aw} x {ah} and {bw} x {bh}): one may be cropped.");
    }
    let (pa, pb) = (aw as u64 * ah as u64, bw as u64 * bh as u64);
    if pa < pb {
        format!("Same picture at a lower resolution ({aw} x {ah} instead of {bw} x {bh}).")
    } else {
        format!("Same picture at a higher resolution ({aw} x {ah} instead of {bw} x {bh}).")
    }
}

// --------------------------------------------------------------------------------------
// Signature cache
// --------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Known {
    size: u64,
    modified: i64,
    kind: u8,
    width: u32,
    height: u32,
    sig: Signature,
}

const CACHE_MAGIC: &[u8; 8] = b"TFPICS1\n";
const CACHE_MAX_ENTRIES: usize = 1_000_000;

fn cache_file(cfg: &ScanConfig) -> Option<PathBuf> {
    cfg.cache_path.as_ref().map(|p| p.with_extension("pics"))
}

fn load_cache(path: &Path) -> HashMap<String, Known> {
    let mut map = HashMap::new();
    let Ok(bytes) = std::fs::read(path) else {
        return map;
    };
    if bytes.len() < 8 || &bytes[..8] != CACHE_MAGIC {
        return map;
    }
    let mut r = &bytes[8..];
    let mut take = |n: usize| -> Option<Vec<u8>> {
        if r.len() < n {
            return None;
        }
        let mut buf = vec![0u8; n];
        r.read_exact(&mut buf).ok()?;
        Some(buf)
    };
    while let Some(len) = take(4) {
        let len = u32::from_le_bytes([len[0], len[1], len[2], len[3]]) as usize;
        if len > 4096 {
            break;
        }
        let (Some(key), Some(rest)) = (take(len), take(8 + 8 + 1 + 4 + 4 + 1 + 1 + 64)) else {
            break;
        };
        let Ok(key) = String::from_utf8(key) else {
            break;
        };
        let u64_at = |o: usize| u64::from_le_bytes(rest[o..o + 8].try_into().unwrap_or([0; 8]));
        let u32_at = |o: usize| u32::from_le_bytes(rest[o..o + 4].try_into().unwrap_or([0; 4]));
        let mut hashes = [0u64; 8];
        for (i, h) in hashes.iter_mut().enumerate() {
            *h = u64_at(27 + i * 8);
        }
        map.insert(
            key,
            Known {
                size: u64_at(0),
                modified: u64_at(8) as i64,
                kind: rest[16],
                width: u32_at(17),
                height: u32_at(21),
                sig: Signature {
                    hashes,
                    flat: rest[25] != 0,
                    mean: rest[26],
                },
            },
        );
    }
    map
}

fn save_cache(path: &Path, map: &HashMap<String, Known>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pics.tmp");
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        out.write_all(CACHE_MAGIC)?;
        for (key, k) in map.iter().take(CACHE_MAX_ENTRIES) {
            out.write_all(&(key.len() as u32).to_le_bytes())?;
            out.write_all(key.as_bytes())?;
            out.write_all(&k.size.to_le_bytes())?;
            out.write_all(&k.modified.to_le_bytes())?;
            out.write_all(&[k.kind])?;
            out.write_all(&k.width.to_le_bytes())?;
            out.write_all(&k.height.to_le_bytes())?;
            out.write_all(&[u8::from(k.sig.flat), k.sig.mean])?;
            for h in k.sig.hashes {
                out.write_all(&h.to_le_bytes())?;
            }
        }
        out.flush()?;
    }
    std::fs::rename(&tmp, path)
}

// --------------------------------------------------------------------------------------
// The scan
// --------------------------------------------------------------------------------------

/// One picture that was read successfully.
#[derive(Clone, Debug)]
struct Pic {
    file: FileEntry,
    width: u32,
    height: u32,
    sig: Signature,
}

/// Groups signatures without chaining: the pictures are visited from largest
/// to smallest; each joins the closest existing group leader within
/// `max_distance`, or becomes a leader itself. Returns, per group, the member
/// indexes (leader first) with their distance to the leader.
pub fn cluster(
    sigs: &[Signature],
    order: &[usize],
    max_distance: u32,
    rotated: bool,
    progress: &Progress,
) -> Vec<Vec<(usize, u32)>> {
    let mut leaders: Vec<usize> = Vec::new(); // index into `groups`
    let mut leader_sigs: Vec<Signature> = Vec::new();
    let mut groups: Vec<Vec<(usize, u32)>> = Vec::new();
    for (n, &i) in order.iter().enumerate() {
        if n % 256 == 0 {
            if progress.cancelled() {
                break;
            }
            progress.add_items(256.min((order.len() - n) as u64));
        }
        let s = &sigs[i];
        let mut best: Option<(usize, u32)> = None;
        for (slot, leader) in leader_sigs.iter().enumerate() {
            let d = s.distance(leader, rotated);
            if d <= max_distance && best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((slot, d));
                if d == 0 {
                    break;
                }
            }
        }
        match best {
            Some((slot, d)) => groups[leaders[slot]].push((i, d)),
            None => {
                leaders.push(groups.len());
                leader_sigs.push(*s);
                groups.push(vec![(i, 0)]);
            }
        }
    }
    groups
}

/// Finds groups of pictures that look alike.
pub fn scan(cfg: &ScanConfig, progress: &Progress) -> ScanResult {
    scan_with(cfg, &Guard::real(), progress)
}

/// [`scan`] with an explicit guard (tests use their own).
pub fn scan_with(cfg: &ScanConfig, guard: &Guard, progress: &Progress) -> ScanResult {
    let mut result = ScanResult::empty(Finder::SimilarImages);
    progress.set_stage_count(3);
    let tree = walk::walk_with(cfg, WalkOptions::ALL, guard, progress);
    result.errors = tree.errors;
    let files: Vec<FileEntry> = tree
        .files
        .into_iter()
        .filter(|f| is_picture(&f.path))
        .collect();
    result.files_scanned = files.len() as u64;
    result.bytes_scanned = files.iter().map(|f| f.size).sum();
    if progress.cancelled() {
        result.cancelled = true;
        return result;
    }

    // Stage 2: read and hash, several pictures at a time.
    let kind = cfg.image_hash;
    let kind_tag = match kind {
        crate::model::ImageHash::Perceptual => 0u8,
        crate::model::ImageHash::Gradient => 1u8,
    };
    let cache_path = cache_file(cfg);
    let mut cache = cache_path.as_deref().map(load_cache).unwrap_or_default();
    progress.set_stage("Reading pictures", files.len() as u64, result.bytes_scanned);
    let next = AtomicUsize::new(0);
    let (bytes_read, hits) = (AtomicU64::new(0), AtomicU64::new(0));
    let found: Mutex<Vec<(usize, Pic)>> = Mutex::new(Vec::new());
    let learned: Mutex<Vec<(String, Known)>> = Mutex::new(Vec::new());
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let cache_ref = &cache;
    std::thread::scope(|scope| {
        for _ in 0..cfg.worker_count().min(files.len().max(1)) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() || progress.cancelled() {
                    break;
                }
                let file = &files[i];
                if i.is_multiple_of(16) {
                    progress.set_current(&file.path);
                }
                let key = norm_path(&file.path.to_string_lossy());
                let known = cache_ref.get(&key).filter(|k| {
                    k.size == file.size && k.modified == file.modified && k.kind == kind_tag
                });
                let outcome = match known {
                    Some(k) => {
                        hits.fetch_add(1, Ordering::Relaxed);
                        Ok((k.width, k.height, k.sig))
                    }
                    None => {
                        // A decoder bug must not take the whole scan down.
                        let path = file.path.clone();
                        std::panic::catch_unwind(move || {
                            decode_sized(&path)
                                .map(|(img, w, h)| (w, h, Signature::of_image(&img, kind)))
                        })
                        .unwrap_or_else(|_| Err("the picture could not be decoded".into()))
                        .inspect(|(w, h, sig)| {
                            bytes_read.fetch_add(file.size, Ordering::Relaxed);
                            if let Ok(mut l) = learned.lock() {
                                l.push((
                                    key.clone(),
                                    Known {
                                        size: file.size,
                                        modified: file.modified,
                                        kind: kind_tag,
                                        width: *w,
                                        height: *h,
                                        sig: *sig,
                                    },
                                ));
                            }
                        })
                    }
                };
                match outcome {
                    Ok((width, height, sig)) => {
                        if let Ok(mut f) = found.lock() {
                            f.push((
                                i,
                                Pic {
                                    file: file.clone(),
                                    width,
                                    height,
                                    sig,
                                },
                            ));
                        }
                    }
                    Err(e) => {
                        if let Ok(mut errs) = errors.lock() {
                            if errs.len() < MAX_ERRORS {
                                errs.push(format!("{}: {e}", file.path.to_string_lossy()));
                            }
                        }
                    }
                }
                progress.add_item();
                progress.add_bytes(file.size);
            });
        }
    });
    result.bytes_read = bytes_read.into_inner();
    result.cache_hits = hits.into_inner();
    result
        .errors
        .extend(errors.into_inner().unwrap_or_default());
    let learned = learned.into_inner().unwrap_or_default();
    if let (Some(path), false) = (&cache_path, learned.is_empty()) {
        cache.extend(learned);
        if let Err(e) = save_cache(path, &cache) {
            result
                .errors
                .push(format!("The picture cache could not be saved: {e}"));
        }
    }
    let mut pics = found.into_inner().unwrap_or_default();
    pics.sort_by_key(|(i, _)| *i);
    let pics: Vec<Pic> = pics.into_iter().map(|(_, p)| p).collect();
    if progress.cancelled() {
        result.cancelled = true;
        return result;
    }

    // Stage 3: compare. Largest pictures first, so each group is led by its best copy.
    progress.set_stage("Comparing pictures", pics.len() as u64, 0);
    let mut order: Vec<usize> = (0..pics.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (&pics[a], &pics[b]);
        (pb.width as u64 * pb.height as u64)
            .cmp(&(pa.width as u64 * pa.height as u64))
            .then(pb.file.size.cmp(&pa.file.size))
            .then_with(|| pa.file.path.cmp(&pb.file.path))
    });
    let sigs: Vec<Signature> = pics.iter().map(|p| p.sig).collect();
    let clusters = cluster(
        &sigs,
        &order,
        phash::max_distance(cfg.similarity),
        cfg.match_rotated,
        progress,
    );
    for mut members in clusters {
        if members.len() < 2 {
            continue;
        }
        // Leader first, then the closest copies.
        members[1..].sort_by_key(|(_, d)| *d);
        let lowest = members
            .iter()
            .map(|(_, d)| phash::similarity(*d))
            .min()
            .unwrap_or(100);
        let items: Vec<Item> = members
            .iter()
            .map(|(i, d)| {
                let p = &pics[*i];
                Item::with_detail(
                    p.file.clone(),
                    Detail::Image {
                        width: p.width,
                        height: p.height,
                        similarity: phash::similarity(*d),
                    },
                )
            })
            .collect();
        let label = if lowest == 100 {
            format!("{} pictures, 100% alike", items.len())
        } else {
            format!("{} pictures, {lowest}% to 100% alike", items.len())
        };
        result.groups.push(Group::new(items, label));
    }
    result.cancelled = progress.cancelled();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ImageHash;
    use crate::selftest::picture;
    use crate::util::file_name;
    use image::imageops;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twinforge-images-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn save(img: &image::RgbImage, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgb8(img.clone())
            .save(path)
            .unwrap();
    }

    fn save_jpeg(img: &image::RgbImage, path: &Path, quality: u8) {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(img)
            .unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn group_names(r: &ScanResult) -> Vec<Vec<String>> {
        let mut all: Vec<Vec<String>> = r
            .groups
            .iter()
            .map(|g| {
                let mut v: Vec<String> = g.items.iter().map(|i| file_name(&i.file.path)).collect();
                v.sort();
                v
            })
            .collect();
        all.sort();
        all
    }

    fn run(cfg: &ScanConfig) -> ScanResult {
        scan_with(cfg, &Guard::none(), &Progress::new())
    }

    #[test]
    fn finds_resized_recompressed_and_rotated_copies() {
        let dir = temp("scan");
        let a = picture(3, 800, 600);
        let b = picture(11, 800, 600);
        save(&a, &dir.join("harbour.png"));
        save_jpeg(
            &imageops::resize(&a, 400, 300, imageops::FilterType::Triangle),
            &dir.join("sub\\..\\harbour-small.jpg"),
            60,
        );
        save(
            &imageops::rotate90(&a),
            &dir.join("turned\\harbour-turned.png"),
        );
        save(&b, &dir.join("meadow.png"));
        save_jpeg(&b, &dir.join("meadow-copy.jpeg"), 85);
        save(&picture(20, 640, 480), &dir.join("alone.bmp"));
        fs::write(dir.join("notes.txt"), b"not a picture").unwrap();
        fs::write(dir.join("broken.jpg"), b"this is not a jpeg at all").unwrap();
        fs::write(dir.join("empty.png"), b"").unwrap();
        fs::write(
            dir.join("cut.png"),
            &fs::read(dir.join("harbour.png")).unwrap()[..200],
        )
        .unwrap();

        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.min_size = 0;
        let r = run(&cfg);
        assert_eq!(
            group_names(&r),
            vec![
                vec!["harbour-small.jpg", "harbour-turned.png", "harbour.png"],
                vec!["meadow-copy.jpeg", "meadow.png"],
            ]
        );
        assert_eq!(r.files_scanned, 9);
        assert_eq!(r.errors.len(), 3, "{:?}", r.errors);
        // The largest picture leads and carries 100%; details are filled in.
        let g = r.groups.iter().find(|g| g.items.len() == 3).unwrap();
        assert!(matches!(
            g.items[0].detail,
            Detail::Image {
                width: 800,
                height: 600,
                similarity: 100
            }
        ));
        assert!(g
            .items
            .iter()
            .all(|i| matches!(i.detail, Detail::Image { similarity, .. } if similarity >= 90)));
        assert!(g.label.starts_with("3 pictures"));

        // Without the rotation option the turned copy drops out.
        cfg.match_rotated = false;
        let r = run(&cfg);
        assert!(group_names(&r).contains(&vec![
            "harbour-small.jpg".to_string(),
            "harbour.png".to_string()
        ]));
        // The gradient method finds the plain copies as well.
        cfg.image_hash = ImageHash::Gradient;
        let r = run(&cfg);
        assert_eq!(r.groups.len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stricter_settings_never_find_more() {
        let dir = temp("slider");
        let a = picture(5, 640, 480);
        save(&a, &dir.join("a.png"));
        save_jpeg(&a, &dir.join("a-low.jpg"), 35);
        let noisy = image::RgbImage::from_fn(640, 480, |x, y| {
            let p = a.get_pixel(x, y);
            let bump = if (x / 40 + y / 40) % 2 == 0 { 22 } else { 0 };
            image::Rgb([p[0].saturating_add(bump), p[1], p[2].saturating_sub(bump)])
        });
        save(&noisy, &dir.join("a-edited.png"));
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let mut last = usize::MAX;
        for s in [60u8, 75, 85, 90, 95, 100] {
            cfg.similarity = s;
            let n: usize = run(&cfg).groups.iter().map(|g| g.items.len()).sum();
            assert!(
                n <= last,
                "similarity {s} found {n}, looser setting found {last}"
            );
            last = n;
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_spares_the_second_read_and_notices_changes() {
        let dir = temp("cache");
        let a = picture(3, 400, 300);
        save(&a, &dir.join("pics\\a.png"));
        save_jpeg(&a, &dir.join("pics\\a.jpg"), 80);
        let mut cfg = ScanConfig::for_paths(&[dir.join("pics")]);
        cfg.cache_path = Some(dir.join("app\\hash-cache.bin"));
        let first = run(&cfg);
        assert_eq!((first.groups.len(), first.cache_hits), (1, 0));
        assert!(first.bytes_read > 0);
        assert!(dir.join("app\\hash-cache.pics").exists());
        let second = run(&cfg);
        assert_eq!(
            (second.groups.len(), second.cache_hits, second.bytes_read),
            (1, 2, 0)
        );
        // A replaced file is read again.
        save(&picture(11, 400, 300), &dir.join("pics\\a.png"));
        let third = run(&cfg);
        assert_eq!(third.cache_hits, 1);
        assert!(third.groups.is_empty());
        // A damaged cache is ignored.
        fs::write(
            dir.join("app\\hash-cache.pics"),
            b"TFPICS1\n\xff\xff\xff\xffgarbage",
        )
        .unwrap();
        assert_eq!(run(&cfg).cache_hits, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancel_and_thumbnails() {
        let dir = temp("thumb");
        save(&picture(3, 640, 360), &dir.join("wide.png"));
        save(&picture(3, 640, 360), &dir.join("wide2.png"));
        let p = Progress::new();
        p.cancel();
        let r = scan_with(&ScanConfig::for_paths(&[&dir]), &Guard::none(), &p);
        assert!(r.cancelled && r.groups.is_empty());

        let t = load_thumbnail(&dir.join("wide.png"), 96).unwrap();
        assert_eq!((t.width, t.height), (96, 54));
        assert_eq!((t.full_width, t.full_height), (640, 360));
        assert_eq!(t.rgba.len(), 96 * 54 * 4);
        let t = load_thumbnail(&dir.join("wide.png"), 2000).unwrap();
        assert_eq!((t.width, t.height), (640, 360));
        fs::write(dir.join("bad.png"), b"nope").unwrap();
        assert!(load_thumbnail(&dir.join("bad.png"), 96).is_none());
        assert!(load_thumbnail(&dir.join("missing.png"), 96).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clustering_does_not_chain_and_is_quick() {
        let sig = |bits: u64| Signature {
            hashes: [bits; 8],
            flat: false,
            mean: 100,
        };
        // a - b differ by 4 bits, b - c by 4 bits, a - c by 8: with a limit of
        // 5, c must not ride into a's group on b's back.
        let (a, b, c) = (sig(0), sig(0b1111), sig(0b1111_1111));
        let groups = cluster(&[a, b, c], &[0, 1, 2], 5, false, &Progress::new());
        assert_eq!(groups, vec![vec![(0, 0), (1, 4)], vec![(2, 0)]]);
        // A member goes to the closest leader, not the first acceptable one.
        let groups = cluster(
            &[sig(0), sig(0b111_1111_1111), sig(0b111_1111_1100)],
            &[0, 1, 2],
            9,
            false,
            &Progress::new(),
        );
        assert_eq!(groups[1], vec![(1, 0), (2, 2)]);

        // 20,000 unrelated signatures: the worst case (every one a leader).
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let sigs: Vec<Signature> = (0..20_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                sig(x)
            })
            .collect();
        let order: Vec<usize> = (0..sigs.len()).collect();
        let started = std::time::Instant::now();
        let groups = cluster(&sigs, &order, 6, true, &Progress::new());
        let secs = started.elapsed().as_secs_f64();
        assert!(groups.len() > 19_900);
        assert!(secs < 60.0, "comparing took {secs:.1} s");
        println!("clustered 20,000 signatures in {secs:.2} s");
    }

    #[test]
    fn difference_hints() {
        let item = |name: &str, size: u64, w: u32, h: u32, sim: u8| {
            Item::with_detail(
                FileEntry {
                    path: PathBuf::from(name),
                    size,
                    ..Default::default()
                },
                Detail::Image {
                    width: w,
                    height: h,
                    similarity: sim,
                },
            )
        };
        let big = item("a.jpg", 5000, 4000, 3000, 100);
        assert!(difference_hint(&item("b.jpg", 900, 1600, 1200, 98), &big)
            .contains("lower resolution (1600 x 1200 instead of 4000 x 3000)"));
        assert!(difference_hint(&big, &item("b.jpg", 900, 1600, 1200, 98))
            .contains("higher resolution"));
        assert!(difference_hint(&item("b.jpg", 900, 4000, 3000, 99), &big)
            .contains("more strongly compressed"));
        assert!(difference_hint(&item("b.png", 9000, 4000, 3000, 100), &big)
            .contains("Different format (PNG and JPEG)"));
        assert!(
            difference_hint(&item("b.jpeg", 5000, 4000, 3000, 100), &big).contains("Identical")
        );
        assert!(difference_hint(&item("b.jpg", 5100, 4000, 3000, 91), &big).contains("91% alike"));
        assert!(difference_hint(&item("b.jpg", 5000, 3000, 4000, 95), &big)
            .contains("Turned on its side"));
        assert!(
            difference_hint(&item("b.jpg", 5000, 4000, 2000, 92), &big).contains("Different shape")
        );
        assert!(difference_hint(&Item::default(), &big).contains("No picture details"));
        assert!(is_picture(Path::new("x.JPEG")) && !is_picture(Path::new("x.txt")));
    }

    /// Speed measurement, run on request:
    /// `cargo test --release --lib images::tests::speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn speed() {
        let dir = temp("speed");
        let count = 200;
        for i in 0..count {
            save_jpeg(
                &picture(i % 40, 1600, 1200),
                &dir.join(format!("p{i:03}.jpg")),
                85,
            );
        }
        let cfg = ScanConfig::for_paths(&[&dir]);
        let started = std::time::Instant::now();
        let r = run(&cfg);
        let secs = started.elapsed().as_secs_f64();
        println!(
            "{count} pictures (1600 x 1200 JPEG, {:.1} MB) in {secs:.2} s = {:.0} pictures/s, {} groups, {} workers",
            r.bytes_scanned as f64 / 1e6,
            count as f64 / secs,
            r.groups.len(),
            cfg.worker_count()
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
