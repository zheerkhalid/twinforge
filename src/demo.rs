//! Invented results and generated pictures for `--demo` and the screenshots.
//! Every name, path and picture here is made up; nothing is read from disk.

// The tables below are plain data; naming every tuple shape would not help.
#![allow(clippy::type_complexity)]

use crate::model::{Detail, FileEntry, Finder, Group, Item, ScanResult, Thumb};
use crate::util::{extension, unix_from_date};
use std::path::{Path, PathBuf};

const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * 1024 * 1024;

/// A time stamp from `(year, month, day, hour, minute)`.
fn at(y: i32, mo: u32, d: u32, h: i64, mi: i64) -> i64 {
    unix_from_date(y, mo, d) + h * 3600 + mi * 60
}

fn file(path: &str, size: u64, modified: i64) -> FileEntry {
    FileEntry {
        path: PathBuf::from(path),
        size,
        modified,
        created: modified - 5 * 86_400,
        is_dir: false,
    }
}

fn is_reference(path: &str) -> bool {
    path.starts_with("E:\\Photo archive\\")
}

fn item(path: &str, size: u64, modified: i64, detail: Detail) -> Item {
    Item {
        file: file(path, size, modified),
        reference: is_reference(path),
        selected: false,
        detail,
    }
}

fn summary(finder: Finder, groups: Vec<Group>, files: u64, bytes: u64, ms: u64) -> ScanResult {
    ScanResult {
        finder,
        groups,
        files_scanned: files,
        bytes_scanned: bytes,
        bytes_read: bytes / 9,
        cache_hits: files / 3,
        elapsed_ms: ms,
        cancelled: false,
        errors: Vec::new(),
    }
}

// --------------------------------------------------------------------------------------
// Duplicates
// --------------------------------------------------------------------------------------

/// `(fingerprint, size, [(path, modified)])`
type DupeRow = (
    &'static str,
    u64,
    &'static [(&'static str, (i32, u32, u32, i64, i64))],
);

const DUPES: &[DupeRow] = &[
    (
        "9c41f0a27be3",
        2_576_980_378,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Backups\\studio-project-backup.zip",
                (2025, 11, 2, 21, 14),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\studio-project-backup.zip",
                (2025, 11, 2, 21, 9),
            ),
        ],
    ),
    (
        "3fa9c1d2e4b7",
        1_288_490_188,
        &[
            (
                "D:\\Photos\\2023\\Lisbon\\Harbour walk.mp4",
                (2023, 9, 17, 18, 42),
            ),
            (
                "D:\\Photos\\Phone import\\VID_20230917_184212.mp4",
                (2023, 9, 19, 8, 3),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\Harbour walk (1).mp4",
                (2024, 1, 6, 13, 27),
            ),
        ],
    ),
    (
        "b07d55e19a3c",
        2_040_109_465,
        &[
            (
                "C:\\Users\\Alex\\Downloads\\rescue-disk-2025.iso",
                (2025, 6, 11, 10, 2),
            ),
            (
                "C:\\Users\\Alex\\Documents\\Tools\\rescue-disk-2025.iso",
                (2025, 6, 11, 10, 40),
            ),
        ],
    ),
    (
        "71e2aa90c4d8",
        817_889_280,
        &[
            (
                "D:\\Photos\\2021\\Birthday\\Garden party.mov",
                (2021, 5, 22, 16, 5),
            ),
            (
                "D:\\Photos\\To sort\\Garden party.mov",
                (2022, 2, 3, 19, 51),
            ),
        ],
    ),
    (
        "d4c8e17f02ab",
        432_316_860,
        &[
            (
                "C:\\Users\\Alex\\Downloads\\photo-suite-setup.exe",
                (2025, 3, 4, 9, 30),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\photo-suite-setup (1).exe",
                (2025, 3, 4, 9, 41),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\photo-suite-setup (2).exe",
                (2025, 8, 20, 17, 12),
            ),
        ],
    ),
    (
        "5a90be3317c6",
        38_797_312,
        &[
            (
                "D:\\Music\\Tidewater Assembly\\Shorelines\\03 Low Sun.flac",
                (2022, 10, 8, 20, 15),
            ),
            (
                "D:\\Music\\Unsorted\\Tidewater Assembly - Low Sun.flac",
                (2023, 1, 14, 11, 48),
            ),
        ],
    ),
    (
        "e83f6610d9a5",
        29_412_608,
        &[
            (
                "E:\\Photo archive\\2022\\Alps\\IMG_2204.CR3",
                (2022, 7, 30, 6, 12),
            ),
            ("D:\\Photos\\2022\\Alps\\IMG_2204.CR3", (2022, 7, 30, 6, 12)),
            ("D:\\Photos\\To sort\\IMG_2204.CR3", (2022, 8, 2, 22, 40)),
        ],
    ),
    (
        "0c7714fbe2d1",
        18_874_368,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Work\\Quarterly review.pptx",
                (2025, 9, 29, 15, 4),
            ),
            (
                "C:\\Users\\Alex\\Documents\\Work\\Archive\\Quarterly review - final.pptx",
                (2025, 9, 30, 8, 55),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\Quarterly review.pptx",
                (2025, 10, 1, 7, 31),
            ),
        ],
    ),
    (
        "a1b6d0739e4f",
        9_122_816,
        &[
            (
                "D:\\Music\\Mira Vale\\Paper Rivers\\07 Glass Morning.mp3",
                (2021, 3, 12, 18, 20),
            ),
            (
                "D:\\Music\\Unsorted\\Mira Vale - Glass Morning.mp3",
                (2021, 3, 12, 18, 20),
            ),
        ],
    ),
    (
        "47dd2c98a016",
        5_210_112,
        &[
            (
                "D:\\Photos\\2023\\Lisbon\\IMG_4471.jpg",
                (2023, 9, 17, 10, 15),
            ),
            (
                "D:\\Photos\\2023\\Lisbon\\IMG_4471 (1).jpg",
                (2023, 9, 21, 20, 2),
            ),
            (
                "D:\\Photos\\Phone import\\20230917_101502.jpg",
                (2023, 9, 19, 8, 3),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\IMG_4471.jpg",
                (2024, 2, 11, 12, 0),
            ),
        ],
    ),
    (
        "f2063b8cd57e",
        2_411_724,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Study\\Thesis draft v7.docx",
                (2024, 5, 18, 23, 46),
            ),
            (
                "C:\\Users\\Alex\\Documents\\Study\\Old\\Thesis draft v7 (copy).docx",
                (2024, 5, 19, 0, 2),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\Thesis draft v7.docx",
                (2024, 5, 20, 9, 10),
            ),
        ],
    ),
    (
        "6be4170a93cd",
        1_153_434,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Taxes\\2024\\statement-march.pdf",
                (2024, 4, 2, 14, 18),
            ),
            (
                "C:\\Users\\Alex\\Downloads\\statement-march.pdf",
                (2024, 4, 2, 14, 16),
            ),
        ],
    ),
    (
        "28f5c6e0b41a",
        98_304,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Household\\Budget 2025.xlsx",
                (2025, 1, 5, 19, 22),
            ),
            (
                "C:\\Users\\Alex\\Documents\\Household\\Budget 2025 - copy.xlsx",
                (2025, 1, 5, 19, 25),
            ),
        ],
    ),
    (
        "93ad7e25f608",
        17_408,
        &[
            (
                "C:\\Users\\Alex\\Documents\\Recipes\\sourdough notes.txt",
                (2023, 12, 3, 8, 44),
            ),
            (
                "C:\\Users\\Alex\\Documents\\Recipes\\Old\\sourdough notes.txt",
                (2023, 11, 26, 17, 30),
            ),
        ],
    ),
];

fn duplicates() -> ScanResult {
    let groups = DUPES
        .iter()
        .map(|(hash, size, files)| {
            let items = files
                .iter()
                .map(|(p, (y, mo, d, h, mi))| {
                    item(p, *size, at(*y, *mo, *d, *h, *mi), Detail::None)
                })
                .collect();
            Group::new(items, format!("BLAKE3 {hash}…"))
        })
        .collect();
    summary(Finder::Duplicates, groups, 48_210, 186 * GB, 41_300)
}

// --------------------------------------------------------------------------------------
// Similar pictures
// --------------------------------------------------------------------------------------

/// `(scene, path, size, width, height, similarity, modified)`
type PicRow = (
    u32,
    &'static str,
    u64,
    u32,
    u32,
    u8,
    (i32, u32, u32, i64, i64),
);

const PICS: &[&[PicRow]] = &[
    &[
        (
            1,
            "E:\\Photo archive\\2023\\Lisbon\\IMG_4471.jpg",
            8_808_038,
            6000,
            4000,
            100,
            (2023, 9, 17, 10, 15),
        ),
        (
            1,
            "D:\\Photos\\2023\\Lisbon\\IMG_4471-edit.jpg",
            7_969_177,
            6000,
            4000,
            95,
            (2023, 9, 24, 21, 30),
        ),
        (
            1,
            "D:\\Photos\\Shared\\lisbon-harbour-web.jpg",
            421_888,
            1600,
            1067,
            98,
            (2023, 10, 2, 9, 12),
        ),
        (
            1,
            "C:\\Users\\Alex\\Downloads\\lisbon-harbour-web (1).jpg",
            421_888,
            1600,
            1067,
            98,
            (2024, 3, 9, 18, 40),
        ),
    ],
    &[
        (
            2,
            "D:\\Photos\\Wallpapers\\dune-evening.png",
            12_582_912,
            3840,
            2160,
            100,
            (2024, 6, 1, 7, 55),
        ),
        (
            2,
            "D:\\Photos\\Wallpapers\\dune-evening.jpg",
            2_202_009,
            3840,
            2160,
            98,
            (2024, 6, 1, 8, 1),
        ),
        (
            2,
            "C:\\Users\\Alex\\Downloads\\dune-evening-1080.webp",
            318_464,
            1920,
            1080,
            97,
            (2024, 6, 3, 22, 14),
        ),
    ],
    &[
        (
            3,
            "D:\\Photos\\2022\\Alps\\IMG_2231.jpg",
            6_920_601,
            4032,
            3024,
            100,
            (2022, 7, 31, 11, 2),
        ),
        (
            103,
            "D:\\Photos\\2022\\Alps\\IMG_2231-turned.jpg",
            6_815_744,
            3024,
            4032,
            97,
            (2022, 8, 4, 19, 26),
        ),
    ],
    &[
        (
            4,
            "D:\\Photos\\2024\\Lake house\\DSC_0188.jpg",
            9_542_041,
            6016,
            4016,
            100,
            (2024, 8, 12, 17, 33),
        ),
        (
            4,
            "D:\\Photos\\2024\\Lake house\\DSC_0189.jpg",
            9_489_612,
            6016,
            4016,
            92,
            (2024, 8, 12, 17, 33),
        ),
        (
            4,
            "D:\\Photos\\2024\\Lake house\\DSC_0190.jpg",
            9_611_878,
            6016,
            4016,
            91,
            (2024, 8, 12, 17, 34),
        ),
    ],
    &[
        (
            5,
            "D:\\Photos\\Scans\\Grandparents 1968.tif",
            48_234_496,
            4800,
            3600,
            100,
            (2019, 2, 16, 14, 8),
        ),
        (
            5,
            "D:\\Photos\\Scans\\Grandparents 1968.jpg",
            3_250_585,
            4800,
            3600,
            98,
            (2019, 2, 16, 14, 20),
        ),
        (
            5,
            "C:\\Users\\Alex\\Documents\\Family\\grandparents-small.jpg",
            268_288,
            1200,
            900,
            95,
            (2020, 12, 24, 10, 0),
        ),
    ],
    &[
        (
            6,
            "D:\\Photos\\2023\\Autumn\\P1050332.jpg",
            5_347_737,
            5184,
            3456,
            100,
            (2023, 10, 28, 15, 47),
        ),
        (
            6,
            "D:\\Photos\\Phone import\\P1050332~2.jpg",
            1_782_579,
            2592,
            1728,
            97,
            (2023, 11, 2, 7, 19),
        ),
    ],
    &[
        (
            7,
            "D:\\Photos\\2021\\Coast\\IMG_0912.png",
            21_495_808,
            4000,
            3000,
            100,
            (2021, 4, 3, 12, 30),
        ),
        (
            7,
            "D:\\Photos\\2021\\Coast\\IMG_0912.jpg",
            4_089_446,
            4000,
            3000,
            98,
            (2021, 4, 3, 12, 30),
        ),
        (
            7,
            "D:\\Photos\\2021\\Coast\\IMG_0912-crop.jpg",
            2_936_012,
            3200,
            2400,
            89,
            (2021, 4, 9, 20, 5),
        ),
    ],
    &[
        (
            8,
            "C:\\Users\\Alex\\Documents\\Garden plan\\orchard-sketch.png",
            1_887_436,
            2480,
            1754,
            100,
            (2025, 2, 8, 16, 12),
        ),
        (
            8,
            "C:\\Users\\Alex\\Downloads\\orchard-sketch (2).png",
            1_887_436,
            2480,
            1754,
            100,
            (2025, 2, 9, 9, 3),
        ),
    ],
];

fn pictures() -> ScanResult {
    let groups = PICS
        .iter()
        .map(|rows| {
            let items: Vec<Item> = rows
                .iter()
                .map(|(_, p, size, w, h, sim, (y, mo, d, hh, mi))| {
                    item(
                        p,
                        *size,
                        at(*y, *mo, *d, *hh, *mi),
                        Detail::Image {
                            width: *w,
                            height: *h,
                            similarity: *sim,
                        },
                    )
                })
                .collect();
            let lowest = rows.iter().map(|r| r.5).min().unwrap_or(100);
            let label = if lowest == 100 {
                format!("{} pictures, 100% alike", items.len())
            } else {
                format!("{} pictures, {lowest}% to 100% alike", items.len())
            };
            Group::new(items, label)
        })
        .collect();
    summary(Finder::SimilarImages, groups, 12_846, 71 * GB, 96_500)
}

// --------------------------------------------------------------------------------------
// Similar music
// --------------------------------------------------------------------------------------

/// `(path, size, artist, title, album, seconds, kbps, year)`
type TrackRow = (
    &'static str,
    u64,
    &'static str,
    &'static str,
    &'static str,
    u32,
    u32,
    i32,
);

const TRACKS: &[&[TrackRow]] = &[
    &[
        (
            "D:\\Music\\Tidewater Assembly\\Shorelines\\03 Low Sun.flac",
            38_797_312,
            "Tidewater Assembly",
            "Low Sun",
            "Shorelines",
            312,
            994,
            2022,
        ),
        (
            "D:\\Music\\Phone sync\\Tidewater Assembly - Low Sun.mp3",
            12_582_912,
            "Tidewater Assembly",
            "Low Sun",
            "Shorelines",
            312,
            320,
            2023,
        ),
        (
            "D:\\Music\\Unsorted\\low sun (remastered).mp3",
            7_549_747,
            "tidewater assembly",
            "Low Sun (Remastered)",
            "",
            313,
            192,
            2024,
        ),
    ],
    &[
        (
            "D:\\Music\\Mira Vale\\Paper Rivers\\07 Glass Morning.flac",
            31_457_280,
            "Mira Vale",
            "Glass Morning",
            "Paper Rivers",
            254,
            987,
            2021,
        ),
        (
            "D:\\Music\\Mira Vale\\Paper Rivers\\07 Glass Morning.mp3",
            9_122_816,
            "Mira Vale",
            "Glass Morning",
            "Paper Rivers",
            254,
            287,
            2021,
        ),
    ],
    &[
        (
            "D:\\Music\\The Glass Orchard\\Night Trains\\01 Platform Nine.m4a",
            8_703_180,
            "The Glass Orchard",
            "Platform Nine",
            "Night Trains",
            228,
            256,
            2020,
        ),
        (
            "D:\\Music\\Compilations\\Evening Drive\\14 Platform Nine.m4a",
            8_598_323,
            "Glass Orchard, The",
            "Platform Nine",
            "Evening Drive",
            229,
            256,
            2022,
        ),
        (
            "C:\\Users\\Alex\\Downloads\\glass orchard - platform nine.ogg",
            5_452_595,
            "The Glass Orchard",
            "Platform Nine [Live]",
            "",
            231,
            160,
            2025,
        ),
    ],
    &[
        (
            "D:\\Music\\North of Ember\\Kindling\\05 Slow Weather.flac",
            27_682_406,
            "North of Ember",
            "Slow Weather",
            "Kindling",
            221,
            1002,
            2019,
        ),
        (
            "D:\\Music\\Unsorted\\North Of Ember - Slow Wether.mp3",
            5_347_737,
            "North Of Ember",
            "Slow Wether",
            "Kindling",
            221,
            192,
            2019,
        ),
    ],
    &[
        (
            "D:\\Music\\Kestrel Lane\\Open Fields\\02 Halfway Home.mp3",
            10_171_187,
            "Kestrel Lane",
            "Halfway Home",
            "Open Fields",
            265,
            320,
            2023,
        ),
        (
            "D:\\Music\\Kestrel Lane\\Open Fields (Deluxe)\\02 Halfway Home.mp3",
            10_171_187,
            "Kestrel Lane",
            "Halfway Home",
            "Open Fields (Deluxe)",
            265,
            320,
            2024,
        ),
        (
            "D:\\Music\\Phone sync\\Kestrel Lane - Halfway Home.opus",
            3_460_300,
            "Kestrel Lane",
            "Halfway Home",
            "Open Fields",
            265,
            128,
            2024,
        ),
    ],
    &[
        (
            "D:\\Music\\Solene Arkwright\\Salt & Stone\\09 Lanterns.wav",
            46_871_347,
            "Solène Arkwright",
            "Lanterns",
            "Salt & Stone",
            266,
            1411,
            2018,
        ),
        (
            "D:\\Music\\Solene Arkwright\\Salt & Stone\\09 Lanterns.mp3",
            6_396_313,
            "Solene Arkwright",
            "Lanterns",
            "Salt and Stone",
            266,
            192,
            2018,
        ),
    ],
    &[
        (
            "D:\\Music\\Podcasts\\Garden Hour\\episode 41.mp3",
            58_720_256,
            "Garden Hour",
            "Episode 41: Pruning in Winter",
            "Garden Hour",
            2936,
            160,
            2025,
        ),
        (
            "C:\\Users\\Alex\\Downloads\\garden-hour-041.mp3",
            58_720_256,
            "Garden Hour",
            "Episode 41 - Pruning in Winter",
            "Garden Hour",
            2936,
            160,
            2025,
        ),
    ],
];

fn music() -> ScanResult {
    let groups = TRACKS
        .iter()
        .map(|rows| {
            let items = rows
                .iter()
                .enumerate()
                .map(|(n, (p, size, artist, title, album, secs, kbps, year))| {
                    item(
                        p,
                        *size,
                        at(
                            *year,
                            3 + n as u32 * 2,
                            9 + n as u32 * 5,
                            18,
                            20 + n as i64 * 7,
                        ),
                        Detail::Audio {
                            artist: artist.to_string(),
                            title: title.to_string(),
                            album: album.to_string(),
                            duration_secs: *secs,
                            bitrate_kbps: *kbps,
                        },
                    )
                })
                .collect();
            Group::new(items, format!("{} - {}", rows[0].2, rows[0].3))
        })
        .collect();
    summary(Finder::SimilarMusic, groups, 6_318, 58 * GB, 14_200)
}

// --------------------------------------------------------------------------------------
// The small finders
// --------------------------------------------------------------------------------------

fn same_name() -> ScanResult {
    let rows: &[(&str, &[(&str, u64, (i32, u32, u32))])] = &[
        (
            "cover.jpg",
            &[
                (
                    "D:\\Music\\Tidewater Assembly\\Shorelines\\cover.jpg",
                    412_672,
                    (2022, 10, 8),
                ),
                (
                    "D:\\Music\\Mira Vale\\Paper Rivers\\cover.jpg",
                    388_096,
                    (2021, 3, 12),
                ),
                (
                    "D:\\Music\\Kestrel Lane\\Open Fields\\cover.jpg",
                    501_760,
                    (2023, 5, 2),
                ),
                (
                    "D:\\Music\\North of Ember\\Kindling\\cover.jpg",
                    297_984,
                    (2019, 9, 20),
                ),
            ],
        ),
        (
            "notes.txt",
            &[
                (
                    "C:\\Users\\Alex\\Documents\\Work\\notes.txt",
                    6_144,
                    (2025, 10, 14),
                ),
                (
                    "C:\\Users\\Alex\\Documents\\Study\\notes.txt",
                    22_528,
                    (2024, 5, 2),
                ),
                (
                    "C:\\Users\\Alex\\Downloads\\notes.txt",
                    1_024,
                    (2025, 7, 30),
                ),
            ],
        ),
        (
            "invoice.pdf",
            &[
                (
                    "C:\\Users\\Alex\\Documents\\Household\\2024\\invoice.pdf",
                    184_320,
                    (2024, 11, 3),
                ),
                (
                    "C:\\Users\\Alex\\Documents\\Household\\2025\\invoice.pdf",
                    201_728,
                    (2025, 2, 17),
                ),
                (
                    "C:\\Users\\Alex\\Downloads\\invoice.pdf",
                    173_056,
                    (2025, 9, 9),
                ),
            ],
        ),
        (
            "IMG_0001.jpg",
            &[
                (
                    "D:\\Photos\\2021\\Coast\\IMG_0001.jpg",
                    4_718_592,
                    (2021, 4, 1),
                ),
                (
                    "D:\\Photos\\2023\\Autumn\\IMG_0001.jpg",
                    5_347_737,
                    (2023, 10, 27),
                ),
            ],
        ),
        (
            "Budget.xlsx",
            &[
                (
                    "C:\\Users\\Alex\\Documents\\Household\\Budget.xlsx",
                    96_256,
                    (2025, 1, 5),
                ),
                (
                    "C:\\Users\\Alex\\Documents\\Household\\Old\\Budget.xlsx",
                    71_680,
                    (2023, 1, 8),
                ),
            ],
        ),
        (
            "setup.exe",
            &[
                (
                    "C:\\Users\\Alex\\Downloads\\scanner-driver\\setup.exe",
                    48_234_496,
                    (2024, 9, 12),
                ),
                (
                    "C:\\Users\\Alex\\Downloads\\label-printer\\setup.exe",
                    12_582_912,
                    (2025, 4, 22),
                ),
            ],
        ),
    ];
    let groups = rows
        .iter()
        .map(|(name, files)| {
            let items = files
                .iter()
                .map(|(p, size, (y, mo, d))| item(p, *size, at(*y, *mo, *d, 12, 0), Detail::None))
                .collect();
            Group::new(items, *name)
        })
        .collect();
    summary(Finder::SameName, groups, 48_210, 186 * GB, 2_900)
}

fn flat(finder: Finder, items: Vec<Item>, files: u64, bytes: u64, ms: u64) -> ScanResult {
    let groups = items.into_iter().map(|i| Group::new(vec![i], "")).collect();
    summary(finder, groups, files, bytes, ms)
}

fn empty_files() -> ScanResult {
    let rows: &[(&str, (i32, u32, u32))] = &[
        (
            "C:\\Users\\Alex\\Documents\\Work\\New Text Document.txt",
            (2025, 10, 2),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\Archive\\minutes-draft.docx",
            (2024, 6, 18),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\references.bib",
            (2024, 3, 9),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\download.part.txt",
            (2025, 8, 1),
        ),
        ("C:\\Users\\Alex\\Downloads\\untitled.csv", (2025, 5, 23)),
        ("D:\\Music\\Playlists\\road trip.m3u", (2022, 7, 15)),
        ("D:\\Photos\\2020\\Winter\\captions.txt", (2020, 1, 30)),
        ("D:\\Photos\\To sort\\list.txt", (2023, 2, 4)),
        ("D:\\Photos\\Scans\\index.html", (2019, 2, 16)),
    ];
    let items = rows
        .iter()
        .map(|(p, (y, mo, d))| item(p, 0, at(*y, *mo, *d, 9, 30), Detail::None))
        .collect();
    flat(Finder::EmptyFiles, items, 48_210, 186 * GB, 2_100)
}

fn empty_folders() -> ScanResult {
    let rows: &[(&str, (i32, u32, u32))] = &[
        ("C:\\Users\\Alex\\Documents\\New folder", (2025, 9, 3)),
        (
            "C:\\Users\\Alex\\Documents\\Work\\Archive\\2019",
            (2019, 12, 20),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\scanner-driver\\logs",
            (2024, 9, 12),
        ),
        ("C:\\Users\\Alex\\Downloads\\New folder (2)", (2025, 1, 19)),
        ("D:\\Music\\Playlists\\old", (2021, 6, 6)),
        ("D:\\Photos\\2019\\New folder", (2019, 8, 11)),
        ("D:\\Photos\\2019\\Export\\web", (2019, 8, 14)),
        ("D:\\Photos\\To sort\\done", (2023, 2, 4)),
    ];
    let items = rows
        .iter()
        .map(|(p, (y, mo, d))| {
            let mut i = item(p, 0, at(*y, *mo, *d, 14, 0), Detail::None);
            i.file.is_dir = true;
            i
        })
        .collect();
    flat(Finder::EmptyFolders, items, 3_942, 0, 1_700)
}

fn broken_shortcuts() -> ScanResult {
    let rows: &[(&str, &str, u64, (i32, u32, u32))] = &[
        (
            "C:\\Users\\Alex\\Documents\\Shortcuts\\Photo Suite.lnk",
            "C:\\Program Files\\Photo Suite 4\\photosuite.exe",
            1_412,
            (2023, 3, 4),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Shortcuts\\Scanner.lnk",
            "C:\\Program Files (x86)\\FlatScan\\scan.exe",
            1_208,
            (2022, 9, 12),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\Shared drive.lnk",
            "D:\\Shared\\Team",
            986,
            (2024, 1, 8),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\Thesis folder.lnk",
            "D:\\Study\\Thesis",
            1_034,
            (2024, 5, 18),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\Label Printer Setup.lnk",
            "C:\\Users\\Alex\\Downloads\\label-printer\\install.exe",
            1_530,
            (2025, 4, 22),
        ),
        (
            "D:\\Photos\\Lisbon best of.lnk",
            "D:\\Photos\\2023\\Lisbon\\Best of",
            1_102,
            (2023, 10, 1),
        ),
    ];
    let items = rows
        .iter()
        .map(|(p, target, size, (y, mo, d))| {
            item(
                p,
                *size,
                at(*y, *mo, *d, 11, 15),
                Detail::Shortcut {
                    target: target.to_string(),
                },
            )
        })
        .collect();
    flat(Finder::BrokenShortcuts, items, 48_210, 186 * GB, 2_300)
}

fn big_files() -> ScanResult {
    let rows: &[(&str, u64, (i32, u32, u32))] = &[
        (
            "C:\\Users\\Alex\\Documents\\Virtual machines\\test-lab.vhdx",
            41 * GB + 300 * MB,
            (2025, 10, 30),
        ),
        (
            "D:\\Photos\\Lightroom backups\\catalog-2025-10.zip",
            18 * GB + 120 * MB,
            (2025, 10, 5),
        ),
        (
            "D:\\Photos\\2024\\Lake house\\Drone footage.mov",
            11 * GB + 700 * MB,
            (2024, 8, 13),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\city-map-pack.zip",
            9 * GB + 410 * MB,
            (2025, 7, 2),
        ),
        (
            "D:\\Photos\\2023\\Lisbon\\Timelapse master.mov",
            7 * GB + 880 * MB,
            (2023, 9, 20),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Backups\\phone-backup-2025-06.tar",
            6 * GB + 240 * MB,
            (2025, 6, 28),
        ),
        (
            "D:\\Music\\Recordings\\rehearsal-2024-11-02.wav",
            4 * GB + 690 * MB,
            (2024, 11, 2),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\course-videos-part2.zip",
            3 * GB + 910 * MB,
            (2025, 2, 14),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\course-videos-part1.zip",
            3 * GB + 870 * MB,
            (2025, 2, 14),
        ),
        (
            "D:\\Photos\\2022\\Alps\\Summit panorama.psb",
            3 * GB + 150 * MB,
            (2022, 8, 9),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Backups\\studio-project-backup.zip",
            2_576_980_378,
            (2025, 11, 2),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\studio-project-backup.zip",
            2_576_980_378,
            (2025, 11, 2),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\rescue-disk-2025.iso",
            2_040_109_465,
            (2025, 6, 11),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Tools\\rescue-disk-2025.iso",
            2_040_109_465,
            (2025, 6, 11),
        ),
        (
            "D:\\Photos\\2021\\Birthday\\Full evening.mp4",
            GB + 820 * MB,
            (2021, 5, 23),
        ),
        (
            "D:\\Photos\\2023\\Lisbon\\Harbour walk.mp4",
            1_288_490_188,
            (2023, 9, 17),
        ),
        (
            "D:\\Photos\\Phone import\\VID_20230917_184212.mp4",
            1_288_490_188,
            (2023, 9, 19),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\Harbour walk (1).mp4",
            1_288_490_188,
            (2024, 1, 6),
        ),
        (
            "D:\\Music\\Recordings\\rehearsal-2024-09-14.wav",
            GB + 90 * MB,
            (2024, 9, 14),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\Lecture recordings\\week-08.mkv",
            980 * MB,
            (2024, 4, 11),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\Lecture recordings\\week-07.mkv",
            955 * MB,
            (2024, 4, 4),
        ),
        (
            "D:\\Photos\\2021\\Birthday\\Garden party.mov",
            817_889_280,
            (2021, 5, 22),
        ),
        (
            "D:\\Photos\\To sort\\Garden party.mov",
            817_889_280,
            (2022, 2, 3),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\font-collection.zip",
            702 * MB,
            (2025, 3, 18),
        ),
        (
            "D:\\Photos\\Scans\\Family album 1.pdf",
            644 * MB,
            (2019, 3, 2),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\Product shoot raw.zip",
            598 * MB,
            (2025, 9, 12),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\photo-suite-setup.exe",
            432_316_860,
            (2025, 3, 4),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\photo-suite-setup (1).exe",
            432_316_860,
            (2025, 3, 4),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\photo-suite-setup (2).exe",
            432_316_860,
            (2025, 8, 20),
        ),
        (
            "D:\\Music\\Podcasts\\Garden Hour\\archive-2024.zip",
            388 * MB,
            (2025, 1, 2),
        ),
    ];
    let items = rows
        .iter()
        .map(|(p, size, (y, mo, d))| item(p, *size, at(*y, *mo, *d, 20, 10), Detail::None))
        .collect();
    flat(Finder::BigFiles, items, 48_210, 186 * GB, 2_400)
}

fn temp_files() -> ScanResult {
    let rows: &[(&str, u64, &str, (i32, u32, u32))] = &[
        (
            "C:\\Users\\Alex\\Downloads\\city-map-pack.zip.crdownload",
            1_932_735_283,
            "Unfinished download",
            (2025, 7, 1),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\photo-suite-crash.dmp",
            412 * MB,
            "Crash dump",
            (2025, 8, 21),
        ),
        (
            "D:\\Photos\\Lightroom backups\\catalog.bak",
            268 * MB,
            "Backup copy",
            (2024, 12, 1),
        ),
        (
            "C:\\Users\\Alex\\Downloads\\course-videos-part3.zip.part",
            141 * MB,
            "Unfinished download",
            (2025, 2, 14),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\Thesis draft v6.docx.bak",
            2_306_867,
            "Backup copy",
            (2024, 5, 10),
        ),
        (
            "D:\\Photos\\2019\\Thumbs.db",
            1_887_436,
            "Thumbnail cache",
            (2019, 8, 14),
        ),
        (
            "D:\\Photos\\2020\\Winter\\Thumbs.db",
            1_153_434,
            "Thumbnail cache",
            (2020, 1, 30),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\export-4f2a.tmp",
            734 * KB,
            "Temporary file",
            (2025, 10, 14),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Household\\Budget 2025.xlsx~",
            96 * KB,
            "Editor backup",
            (2025, 1, 5),
        ),
        (
            "D:\\Music\\.DS_Store",
            10 * KB,
            "Desktop cache from another system",
            (2023, 6, 9),
        ),
        (
            "D:\\Photos\\Shared\\.DS_Store",
            8 * KB,
            "Desktop cache from another system",
            (2023, 10, 2),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Work\\~$arterly review.pptx",
            165,
            "Office lock file",
            (2025, 9, 29),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Study\\~$esis draft v7.docx",
            162,
            "Office lock file",
            (2024, 5, 18),
        ),
        (
            "C:\\Users\\Alex\\Documents\\Recipes\\.sourdough notes.txt.swp",
            12 * KB,
            "Editor swap file",
            (2023, 12, 3),
        ),
    ];
    let items = rows
        .iter()
        .map(|(p, size, note, (y, mo, d))| {
            item(
                p,
                *size,
                at(*y, *mo, *d, 8, 45),
                Detail::Note(note.to_string()),
            )
        })
        .collect();
    flat(Finder::TempFiles, items, 48_210, 186 * GB, 2_200)
}

/// An invented, realistic-looking result for a finder.
pub fn result(finder: Finder) -> ScanResult {
    match finder {
        Finder::Duplicates => duplicates(),
        Finder::SimilarImages => pictures(),
        Finder::SimilarMusic => music(),
        Finder::SameName => same_name(),
        Finder::EmptyFiles => empty_files(),
        Finder::EmptyFolders => empty_folders(),
        Finder::BrokenShortcuts => broken_shortcuts(),
        Finder::BigFiles => big_files(),
        Finder::TempFiles => temp_files(),
    }
}

// --------------------------------------------------------------------------------------
// Generated pictures
// --------------------------------------------------------------------------------------

/// The scene number, shape and per-file variation for an invented path.
fn scene_for(path: &Path) -> Option<(u32, f32, u32)> {
    let text = path.to_string_lossy();
    for group in PICS {
        for (n, row) in group.iter().enumerate() {
            if row.1 == text {
                return Some((row.0, row.3 as f32 / row.4 as f32, n as u32));
            }
        }
    }
    if !crate::images::is_picture(path) {
        return None;
    }
    // Any other invented picture: copies ("name (1)", "name-small") share a scene.
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let base: String = stem
        .split([' ', '(', '-', '~'])
        .next()
        .unwrap_or("")
        .to_string();
    let hash = base
        .bytes()
        .fold(17u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    Some((10 + hash % 40, 1.5, stem.len() as u32 % 3))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn hue(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let (i, f) = (h.floor() as i32, h - h.floor());
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// One pixel of a generated landscape: sky, sun, three ridges, water.
fn scene_pixel(scene: u32, u: f32, v: f32) -> [f32; 3] {
    let s = scene as f32;
    let sky_hue = (s * 0.137 + 0.55).fract();
    let warm = (s * 0.31).fract();
    let top = hue(sky_hue, 0.55, 0.55 + 0.25 * warm);
    let low = hue(sky_hue + 0.42 + 0.1 * warm, 0.45, 1.0);
    let horizon = 0.58 + 0.08 * (s * 0.7).sin();
    let mut c = mix(top, low, (v / horizon).powf(1.4));
    // Sun
    let (sx, sy) = (
        0.2 + 0.6 * (s * 0.618).fract(),
        horizon - 0.12 - 0.2 * (s * 0.27).fract(),
    );
    let d = ((u - sx).powi(2) * 1.6 + (v - sy).powi(2)).sqrt();
    c = mix(c, [1.0, 0.97, 0.85], (1.0 - d / 0.07).clamp(0.0, 1.0));
    c = mix(c, [1.0, 0.9, 0.7], (0.35 - d).clamp(0.0, 1.0) * 0.5);
    // Ridges, far to near
    for layer in 0..3 {
        let l = layer as f32;
        let ridge = horizon - 0.1
            + l * 0.09
            + 0.05 * ((u * (3.0 + l * 2.0 + s % 3.0) + s * (1.3 + l)).sin())
            + 0.025 * ((u * (9.0 + l * 4.0) + s * 2.1).sin());
        if v > ridge {
            let shade = hue(sky_hue + 0.3 + 0.07 * l, 0.35 + 0.1 * l, 0.5 - 0.13 * l);
            c = mix(c, shade, 0.82 + 0.06 * l);
        }
    }
    // Water or meadow at the bottom
    let shore = horizon + 0.22;
    if v > shore {
        let t = (v - shore) / (1.0 - shore);
        let ripple = ((v * 90.0 + (u * 14.0).sin() * 2.0).sin() * 0.5 + 0.5) * 0.12;
        let base = if scene.is_multiple_of(2) {
            mix(low, top, 0.6 + t * 0.3)
        } else {
            hue(0.28 + 0.05 * warm, 0.5, 0.55 - 0.2 * t)
        };
        c = mix(base, [1.0, 1.0, 1.0], ripple * (1.0 - t));
    }
    c
}

/// A generated picture for an invented path (never reads the disk).
pub fn thumbnail(path: &Path, max_side: u32) -> Option<Thumb> {
    let (scene, aspect, variant) = scene_for(path)?;
    let turned = (100..200).contains(&scene);
    let scene = if turned { scene - 100 } else { scene };
    let max_side = max_side.clamp(8, 1024);
    let (w, h) = if aspect >= 1.0 {
        (max_side, ((max_side as f32 / aspect).round() as u32).max(1))
    } else {
        (((max_side as f32 * aspect).round() as u32).max(1), max_side)
    };
    // Each copy differs a little: slightly brighter, or cooler.
    let gain = 1.0 + 0.05 * (variant % 3) as f32;
    let tint = if extension(path) == "png" {
        0.0
    } else {
        0.015 * variant as f32
    };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            // A picture that was turned on its side shows the scene rotated.
            let (u, v) = if turned { (v, 1.0 - u) } else { (u, v) };
            let c = scene_pixel(scene, u, v);
            let px = [
                (c[0] * gain + tint).clamp(0.0, 1.0),
                (c[1] * gain).clamp(0.0, 1.0),
                (c[2] * gain - tint).clamp(0.0, 1.0),
            ];
            rgba.extend_from_slice(&[
                (px[0] * 255.0) as u8,
                (px[1] * 255.0) as u8,
                (px[2] * 255.0) as u8,
                255,
            ]);
        }
    }
    // The "original size" of pictures from the table.
    let text = path.to_string_lossy();
    let (full_width, full_height) = PICS
        .iter()
        .flat_map(|g| g.iter())
        .find(|r| r.1 == text)
        .map(|r| (r.3, r.4))
        .unwrap_or((w * 8, h * 8));
    Some(Thumb {
        width: w,
        height: h,
        rgba,
        full_width,
        full_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select;

    #[test]
    fn every_finder_has_a_well_formed_result() {
        for finder in Finder::ALL {
            let r = result(finder);
            assert_eq!(r.finder, finder);
            assert!(r.groups.len() >= 6, "{finder:?}");
            for g in &r.groups {
                if finder.is_grouped() {
                    assert!(g.items.len() >= 2, "{finder:?}");
                    assert!(g.items.iter().any(|i| !i.reference));
                } else {
                    assert_eq!(g.items.len(), 1, "{finder:?}");
                }
                for i in &g.items {
                    assert!(!i.selected);
                    let p = i.file.path.to_string_lossy();
                    assert!(
                        p.starts_with("C:\\Users\\Alex\\")
                            || p.starts_with("D:\\Photos\\")
                            || p.starts_with("D:\\Music\\")
                            || p.starts_with("E:\\Photo archive\\"),
                        "{p}"
                    );
                    assert_eq!(i.reference, p.starts_with("E:\\"));
                    assert!(i.file.modified > 1_500_000_000);
                }
            }
            assert!(select::is_valid(&r.groups));
            assert_eq!(result(finder), r, "deterministic");
        }
        // Exact duplicates really have equal sizes; the biggest files are in order.
        for g in &result(Finder::Duplicates).groups {
            assert!(g.items.iter().all(|i| i.file.size == g.items[0].file.size));
        }
        let big = result(Finder::BigFiles);
        assert!(big
            .groups
            .windows(2)
            .all(|w| w[0].items[0].file.size >= w[1].items[0].file.size));
        assert!(result(Finder::EmptyFolders)
            .groups
            .iter()
            .all(|g| g.items[0].file.is_dir));
        assert!(result(Finder::EmptyFiles)
            .groups
            .iter()
            .all(|g| g.items[0].file.size == 0));
    }

    #[test]
    fn generated_pictures() {
        let a = Path::new("D:\\Photos\\Wallpapers\\dune-evening.png");
        let t = thumbnail(a, 96).unwrap();
        assert_eq!((t.width, t.height), (96, 54));
        assert_eq!(t.rgba.len(), 96 * 54 * 4);
        assert_eq!((t.full_width, t.full_height), (3840, 2160));
        assert_eq!(thumbnail(a, 96), Some(t.clone()), "deterministic");
        // A copy of the same scene looks alike; another scene does not.
        let diff = |x: &Thumb, y: &Thumb| -> f64 {
            x.rgba
                .iter()
                .zip(&y.rgba)
                .map(|(p, q)| (*p as f64 - *q as f64).abs())
                .sum::<f64>()
                / x.rgba.len() as f64
        };
        let copy = thumbnail(Path::new("D:\\Photos\\Wallpapers\\dune-evening.jpg"), 96).unwrap();
        let other = thumbnail(Path::new("D:\\Photos\\2021\\Coast\\IMG_0912.png"), 96).unwrap();
        assert_eq!((other.width, other.height), (96, 72));
        let other_wide =
            thumbnail(Path::new("D:\\Photos\\2024\\Lake house\\DSC_0188.jpg"), 96).unwrap();
        assert!(diff(&t, &copy) < 12.0, "{}", diff(&t, &copy));
        assert!(other_wide.rgba.len() != t.rgba.len() || diff(&t, &other_wide) > 12.0);
        // A turned copy is taller than wide.
        let turned =
            thumbnail(Path::new("D:\\Photos\\2022\\Alps\\IMG_2231-turned.jpg"), 96).unwrap();
        assert_eq!((turned.width, turned.height), (72, 96));
        // Pictures outside the table still get a scene; other files do not.
        assert!(thumbnail(Path::new("D:\\Photos\\2023\\Lisbon\\IMG_4471 (1).jpg"), 64).is_some());
        assert!(thumbnail(Path::new("D:\\Music\\track.mp3"), 64).is_none());
    }
}
