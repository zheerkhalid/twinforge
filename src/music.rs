//! Similar music: reads the tags of audio files and groups tracks whose
//! artist, title, album and length say they are the same recording, even when
//! the files themselves differ (other format, other bitrate, other spelling).
//! The sound itself is not analysed.

use crate::fuzzy;
use crate::model::{
    Detail, FileEntry, Finder, Group, Item, MusicMatch, Progress, ScanConfig, ScanResult,
};
use crate::sys::Guard;
use crate::util::extension;
use crate::walk::{self, WalkOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// File types that are read as music.
pub const EXTENSIONS: [&str; 14] = [
    "mp3", "flac", "m4a", "m4b", "aac", "ogg", "oga", "opus", "wav", "wma", "aiff", "aif", "ape",
    "wv",
];

/// Lengths may differ by this many seconds and still count as the same.
const LENGTH_TOLERANCE: u32 = 3;
const MAX_ERRORS: usize = 200;

pub fn is_audio(path: &Path) -> bool {
    EXTENSIONS.contains(&extension(path).as_str())
}

/// What the tags (or, failing that, the file name) say about one track.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: u32,
    pub bitrate_kbps: u32,
}

/// Fills missing artist / title from a file name such as `Artist - Title.mp3`.
pub fn fill_from_name(track: &mut Track, path: &Path) {
    if !track.title.trim().is_empty() {
        return;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Drop a leading track number: "07 Low Sun", "07. Low Sun", "07 - Low Sun".
    let digits = stem.chars().take_while(|c| c.is_ascii_digit()).count();
    let rest = if (1..=3).contains(&digits) && stem.len() > digits {
        let tail = stem[digits..].trim_start_matches(['.', ' ', '-', '_']);
        if tail.is_empty() {
            stem.as_str()
        } else {
            tail
        }
    } else {
        stem.as_str()
    };
    match rest.split_once(" - ") {
        Some((artist, title)) if !artist.trim().is_empty() && !title.trim().is_empty() => {
            if track.artist.trim().is_empty() {
                track.artist = artist.trim().to_string();
            }
            track.title = title.trim().to_string();
        }
        _ => track.title = rest.trim().to_string(),
    }
}

/// Reads the tags and audio properties of one file.
pub fn read_track(path: &Path) -> Result<Track, String> {
    let tagged = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    let props = tagged.properties();
    let mut track = Track {
        duration_secs: props.duration().as_secs_f64().round() as u32,
        bitrate_kbps: props
            .audio_bitrate()
            .or(props.overall_bitrate())
            .unwrap_or(0),
        ..Default::default()
    };
    // The file's main tag first, then any other tag it carries.
    let tags = tagged.primary_tag().into_iter().chain(tagged.tags().iter());
    for tag in tags {
        let pick = |current: &mut String, value: Option<std::borrow::Cow<'_, str>>| {
            if current.is_empty() {
                if let Some(v) = value {
                    let v = v.trim().trim_matches('\0').trim();
                    if !v.is_empty() {
                        *current = v.to_string();
                    }
                }
            }
        };
        pick(&mut track.title, tag.title());
        pick(&mut track.artist, tag.artist());
        pick(&mut track.album, tag.album());
    }
    fill_from_name(&mut track, path);
    Ok(track)
}

/// A track prepared for comparison.
#[derive(Clone, Debug)]
struct Prepared {
    title: String,
    artist: String,
    album: String,
    secs: u32,
}

fn prepare(t: &Track) -> Prepared {
    Prepared {
        title: fuzzy::normalize(&t.title),
        artist: fuzzy::normalize(&t.artist),
        album: fuzzy::normalize(&t.album),
        secs: t.duration_secs,
    }
}

/// Do two tracks count as the same music under these rules?
fn same_music(a: &Prepared, b: &Prepared, rules: &MusicMatch) -> bool {
    if rules.duration && a.secs > 0 && b.secs > 0 && a.secs.abs_diff(b.secs) > LENGTH_TOLERANCE {
        return false;
    }
    let need = rules.fuzziness.clamp(50, 100);
    let field = |on: bool, x: &str, y: &str| {
        // An empty tag is never proof of anything.
        !on || (!x.is_empty() && !y.is_empty() && fuzzy::similarity_normalized(x, y) >= need)
    };
    field(rules.title, &a.title, &b.title)
        && field(rules.artist, &a.artist, &b.artist)
        && field(rules.album, &a.album, &b.album)
}

/// Groups tracks without chaining: each track joins the first group whose
/// leader it matches, or starts a group. To stay fast on large libraries only
/// tracks whose most important enabled tag starts with the same two letters
/// are compared, so a typo in the first two letters hides a match.
pub fn cluster(tracks: &[Track], rules: &MusicMatch, progress: &Progress) -> Vec<Vec<usize>> {
    if !(rules.title || rules.artist || rules.album) {
        return Vec::new();
    }
    let prepared: Vec<Prepared> = tracks.iter().map(prepare).collect();
    let key_of = |p: &Prepared| -> Option<String> {
        let text = if rules.title {
            &p.title
        } else if rules.artist {
            &p.artist
        } else {
            &p.album
        };
        // Word order may differ ("artist title" in a title tag), so the key
        // comes from the alphabetically first word.
        let word = text.split(' ').min()?;
        if word.is_empty() {
            return None;
        }
        Some(word.chars().take(2).collect())
    };
    let mut blocks: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, p) in prepared.iter().enumerate() {
        if let Some(key) = key_of(p) {
            blocks.entry(key).or_default().push(i);
        }
    }
    let mut keys: Vec<&String> = blocks.keys().collect();
    keys.sort();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for key in keys {
        if progress.cancelled() {
            break;
        }
        let members = &blocks[key];
        let mut local: Vec<Vec<usize>> = Vec::new();
        for &i in members {
            match local
                .iter_mut()
                .find(|g| same_music(&prepared[i], &prepared[g[0]], rules))
            {
                Some(g) => g.push(i),
                None => local.push(vec![i]),
            }
        }
        progress.add_items(members.len() as u64);
        groups.extend(local.into_iter().filter(|g| g.len() >= 2));
    }
    groups
}

/// Finds groups of tracks whose tags say they are the same music.
pub fn scan(cfg: &ScanConfig, progress: &Progress) -> ScanResult {
    scan_with(cfg, &Guard::real(), progress)
}

/// [`scan`] with an explicit guard (tests use their own).
pub fn scan_with(cfg: &ScanConfig, guard: &Guard, progress: &Progress) -> ScanResult {
    let mut result = ScanResult::empty(Finder::SimilarMusic);
    progress.set_stage_count(3);
    let tree = walk::walk_with(cfg, WalkOptions::ALL, guard, progress);
    result.errors = tree.errors;
    let mut files: Vec<FileEntry> = tree
        .files
        .into_iter()
        .filter(|f| is_audio(&f.path))
        .collect();
    // A fixed order makes the result independent of how the disk lists folders.
    files.sort_by(|a, b| a.path.cmp(&b.path));
    result.files_scanned = files.len() as u64;
    result.bytes_scanned = files.iter().map(|f| f.size).sum();
    if progress.cancelled() {
        result.cancelled = true;
        return result;
    }

    progress.set_stage("Reading tags", files.len() as u64, 0);
    let next = AtomicUsize::new(0);
    let read: Mutex<Vec<(usize, Track)>> = Mutex::new(Vec::new());
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..cfg.worker_count().min(files.len().max(1)) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() || progress.cancelled() {
                    break;
                }
                let path = &files[i].path;
                if i.is_multiple_of(32) {
                    progress.set_current(path);
                }
                let owned = path.clone();
                let outcome = std::panic::catch_unwind(move || read_track(&owned))
                    .unwrap_or_else(|_| Err("the tags could not be read".into()));
                match outcome {
                    Ok(track) => {
                        if let Ok(mut r) = read.lock() {
                            r.push((i, track));
                        }
                    }
                    Err(e) => {
                        if let Ok(mut errs) = errors.lock() {
                            if errs.len() < MAX_ERRORS {
                                errs.push(format!("{}: {e}", path.to_string_lossy()));
                            }
                        }
                    }
                }
                progress.add_item();
            });
        }
    });
    result
        .errors
        .extend(errors.into_inner().unwrap_or_default());
    let mut read = read.into_inner().unwrap_or_default();
    read.sort_by_key(|(i, _)| *i);
    if progress.cancelled() {
        result.cancelled = true;
        return result;
    }

    progress.set_stage("Comparing tracks", read.len() as u64, 0);
    let tracks: Vec<Track> = read.iter().map(|(_, t)| t.clone()).collect();
    for members in cluster(&tracks, &cfg.music, progress) {
        let mut items: Vec<Item> = members
            .iter()
            .map(|&m| {
                let t = &tracks[m];
                Item::with_detail(
                    files[read[m].0].clone(),
                    Detail::Audio {
                        artist: t.artist.clone(),
                        title: t.title.clone(),
                        album: t.album.clone(),
                        duration_secs: t.duration_secs,
                        bitrate_kbps: t.bitrate_kbps,
                    },
                )
            })
            .collect();
        // Best quality first.
        let quality = |i: &Item| match &i.detail {
            Detail::Audio { bitrate_kbps, .. } => *bitrate_kbps,
            _ => 0,
        };
        items.sort_by(|a, b| {
            quality(b)
                .cmp(&quality(a))
                .then(b.file.size.cmp(&a.file.size))
                .then_with(|| a.file.path.cmp(&b.file.path))
        });
        let lead = &tracks[members[0]];
        let label = if lead.artist.is_empty() {
            lead.title.clone()
        } else {
            format!("{} - {}", lead.artist, lead.title)
        };
        result.groups.push(Group::new(items, label));
    }
    result.cancelled = progress.cancelled();
    result
}

/// Writes a short, silent WAV file whose RIFF INFO chunk carries these tags.
/// Used by the unit tests and the self-test to make tagged audio without
/// shipping any.
pub fn write_test_wav(
    path: &Path,
    artist: &str,
    title: &str,
    album: &str,
    seconds: u32,
) -> Result<(), String> {
    const RATE: u32 = 8000;
    let samples = vec![0x80u8; (seconds * RATE) as usize]; // 8-bit silence
    let chunk = |id: &[u8; 4], data: &[u8]| -> Vec<u8> {
        let mut c = Vec::with_capacity(data.len() + 9);
        c.extend_from_slice(id);
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&1u16.to_le_bytes()); // mono
    fmt.extend_from_slice(&RATE.to_le_bytes());
    fmt.extend_from_slice(&RATE.to_le_bytes()); // bytes per second
    fmt.extend_from_slice(&1u16.to_le_bytes()); // block align
    fmt.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
    let mut info = b"INFO".to_vec();
    for (id, value) in [(b"INAM", title), (b"IART", artist), (b"IPRD", album)] {
        if !value.is_empty() {
            let mut text = value.as_bytes().to_vec();
            text.push(0);
            info.extend(chunk(id, &text));
        }
    }
    let mut body = b"WAVE".to_vec();
    body.extend(chunk(b"fmt ", &fmt));
    body.extend(chunk(b"data", &samples));
    if info.len() > 4 {
        body.extend(chunk(b"LIST", &info));
    }
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend(body);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, file).map_err(|e| format!("{}: {e}", path.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::file_name;
    use std::fs;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twinforge-music-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(r: &ScanResult) -> Vec<Vec<String>> {
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
    fn generated_wav_round_trips_through_the_reader() {
        let dir = temp("wav");
        let path = dir.join("deep\\Song.wav");
        write_test_wav(&path, "Tidewater", "Low Sun", "Shorelines", 3).unwrap();
        let t = read_track(&path).unwrap();
        assert_eq!(
            (t.artist.as_str(), t.title.as_str(), t.album.as_str()),
            ("Tidewater", "Low Sun", "Shorelines")
        );
        assert_eq!(t.duration_secs, 3);
        assert!(t.bitrate_kbps > 0);
        // Without tags the file name is used.
        let bare = dir.join("07 - Paper Kites - Glass Morning.wav");
        write_test_wav(&bare, "", "", "", 1).unwrap();
        let t = read_track(&bare).unwrap();
        assert_eq!(
            (t.artist.as_str(), t.title.as_str()),
            ("Paper Kites", "Glass Morning")
        );
        assert!(read_track(&dir.join("missing.wav")).is_err());
        fs::write(dir.join("junk.mp3"), b"not audio").unwrap();
        assert!(read_track(&dir.join("junk.mp3")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_name_fallback() {
        let named = |name: &str, artist: &str| {
            let mut t = Track {
                artist: artist.into(),
                ..Default::default()
            };
            fill_from_name(&mut t, Path::new(name));
            (t.artist, t.title)
        };
        assert_eq!(
            named("Tidewater - Low Sun.mp3", ""),
            ("Tidewater".into(), "Low Sun".into())
        );
        assert_eq!(
            named("03. Low Sun.flac", ""),
            (String::new(), "Low Sun".into())
        );
        assert_eq!(
            named("03 Low Sun.flac", "Tidewater"),
            ("Tidewater".into(), "Low Sun".into())
        );
        assert_eq!(
            named("X - Low Sun.flac", "Tidewater"),
            ("Tidewater".into(), "Low Sun".into())
        );
        assert_eq!(named("1999.mp3", ""), (String::new(), "1999".into()));
        // A title that is already there is left alone.
        let mut t = Track {
            title: "Kept".into(),
            ..Default::default()
        };
        fill_from_name(&mut t, Path::new("Other - Name.mp3"));
        assert_eq!((t.artist.as_str(), t.title.as_str()), ("", "Kept"));
    }

    #[test]
    fn groups_the_same_track_and_nothing_else() {
        let dir = temp("scan");
        write_test_wav(
            &dir.join("a\\Low Sun.wav"),
            "Tidewater",
            "Low Sun",
            "Shorelines",
            2,
        )
        .unwrap();
        write_test_wav(
            &dir.join("b\\low sun remaster.wav"),
            "TIDEWATER",
            "Low Sun (Remastered)",
            "Shorelines Deluxe",
            2,
        )
        .unwrap();
        write_test_wav(&dir.join("b\\typo.wav"), "Tidewatr", "Low Sun", "", 3).unwrap();
        write_test_wav(
            &dir.join("c\\other title.wav"),
            "Tidewater",
            "High Moon",
            "Shorelines",
            2,
        )
        .unwrap();
        write_test_wav(
            &dir.join("c\\other artist.wav"),
            "Paper Kites",
            "Low Sun",
            "Atlas",
            2,
        )
        .unwrap();
        write_test_wav(
            &dir.join("c\\long version.wav"),
            "Tidewater",
            "Low Sun",
            "Shorelines",
            9,
        )
        .unwrap();
        write_test_wav(&dir.join("d\\untagged one.wav"), "", "", "", 2).unwrap();
        write_test_wav(&dir.join("d\\untagged two.wav"), "", "", "", 2).unwrap();
        fs::write(dir.join("d\\broken.mp3"), b"nothing useful").unwrap();
        fs::write(dir.join("d\\notes.txt"), b"not audio").unwrap();

        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let r = run(&cfg);
        assert_eq!(
            names(&r),
            vec![vec!["Low Sun.wav", "low sun remaster.wav", "typo.wav"]]
        );
        assert_eq!(r.files_scanned, 9);
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.groups[0].label, "Tidewater - Low Sun");
        assert!(matches!(
            &r.groups[0].items[0].detail,
            Detail::Audio { title, duration_secs, .. } if title.starts_with("Low Sun") && *duration_secs >= 2
        ));

        // Length no longer matters: the long version joins.
        cfg.music.duration = false;
        assert_eq!(run(&cfg).groups[0].items.len(), 4);
        // Exact text only: the misspelt artist drops out.
        cfg.music.duration = true;
        cfg.music.fuzziness = 100;
        assert_eq!(
            names(&run(&cfg)),
            vec![vec!["Low Sun.wav", "low sun remaster.wav"]]
        );
        // Album must agree too: "Shorelines Deluxe" is not close enough at 85.
        cfg.music.fuzziness = 85;
        cfg.music.album = true;
        assert!(run(&cfg).groups.is_empty());
        // Title only: the other artist's "Low Sun" joins the group.
        cfg.music.album = false;
        cfg.music.artist = false;
        assert_eq!(run(&cfg).groups[0].items.len(), 4);
        // Nothing to compare by: no groups at all.
        cfg.music.title = false;
        assert!(run(&cfg).groups.is_empty());

        let p = Progress::new();
        p.cancel();
        assert!(scan_with(&ScanConfig::for_paths(&[&dir]), &Guard::none(), &p).cancelled);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clustering_rules_and_scale() {
        let t = |artist: &str, title: &str, secs: u32| Track {
            artist: artist.into(),
            title: title.into(),
            duration_secs: secs,
            ..Default::default()
        };
        let rules = MusicMatch::default();
        // Unknown length on one side is ignored; unknown artist is not.
        let tracks = vec![
            t("Tidewater", "Low Sun", 200),
            t("Tidewater", "Low Sun", 0),
            t("", "Low Sun", 200),
            t("Tidewater", "Low Sun", 204),
        ];
        assert_eq!(cluster(&tracks, &rules, &Progress::new()), vec![vec![0, 1]]);
        // Tracks with no title never group, however many there are.
        let blanks = vec![t("", "", 100), t("", "", 100), t("", "", 100)];
        assert!(cluster(&blanks, &rules, &Progress::new()).is_empty());

        // 50,000 tracks, 25,000 pairs.
        let mut big = Vec::new();
        for i in 0..25_000u32 {
            let title = format!(
                "{} song number {i}",
                ["amber", "birch", "cedar", "dune"][i as usize % 4]
            );
            big.push(t("Some Artist", &title, 180 + i % 60));
            big.push(t("some artist", &title.to_uppercase(), 181 + i % 60));
        }
        let started = std::time::Instant::now();
        let groups = cluster(&big, &rules, &Progress::new());
        let secs = started.elapsed().as_secs_f64();
        assert_eq!(groups.len(), 25_000);
        assert!(groups.iter().all(|g| g.len() == 2));
        println!("clustered 50,000 tracks in {secs:.2} s");
        assert!(secs < 120.0);
    }
}
