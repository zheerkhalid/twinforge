# Changelog

All notable changes to TwinForge are listed here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the version numbers follow [Semantic Versioning](https://semver.org/).

## [1.1.0] - 2026-11-24

### Added

- **Kurdish, Arabic, Persian and Hebrew names are shown correctly.** File and folder names in right-to-left scripts are now drawn joined and in the right order, the same way File Explorer shows them: in the results, the compare panel, the folder list, the confirmation, the report and the history.
- **"Not a match": hide a group for good.** Right-click a group (or use the button in the compare panel) to hide it. The group leaves the list and stays hidden in later scans, until a new file joins it. Nothing is deleted. *Settings → Hidden groups → Show them again* brings everything back.
- **Large compare window for pictures.** *Enlarge* in the compare panel opens the two pictures side by side at a size where differences can be seen, with *Keep this one*, *Keep the other* and *Not a match*. Up and Down move through the list while it is open.
- **iPhone and camera pictures.** Similar pictures now also reads HEIC/HEIF, AVIF, JPEG XL and camera RAW files (DNG, CR2, CR3, NEF, ARW, ORF, RW2, RAF, PEF, SRW, NRW), using the picture codecs of Windows. Some of these need a free extension from the Microsoft Store (HEIF Image Extensions, AV1 Video Extension, Raw Image Extension); without it the file is listed as "not read".
- **New keep-rule: "Keep the original name".** Keeps the file whose name does not look like a copy, such as `photo (1).jpg`, `report - Copy.docx` or `Copy of budget.xlsx`, and selects the others. Also on the command line: `--keep original-name`.
- **"Keep the copies in this folder".** Right-click a file: in every group that has a copy in that folder, that copy stays and the others are selected.
- **Right-click menu for groups:** *Select all but the first*, *Clear this group*, *Hide this group from now on*.
- **Drag and drop:** drop folders on the window to add them to the places to scan (a dropped file stands for its folder).
- **Keyboard:** Left and Right close and open the highlighted group.

### Changed

- Similar music treats Kurdish, Arabic and Persian tags as the same text when they only differ in how they were typed (ي / ی, ك / ک, ە / ه, with or without vowel marks), and reads Arabic and Persian digits as numbers.
- A picture in a variant the built-in decoder does not support gets a second try with the Windows codecs before it is reported as unreadable. Damaged files are still reported, never half-read.
- The "Everything in a folder" menu and the "scanning now" line show right-to-left paths correctly too.
- `scripts\package.ps1` and the installer script build from `CARGO_TARGET_DIR` when it is set.

### Fixed

- With the "With selection" or "Without selection" filter on, the list did not follow Ctrl+A, Ctrl+I, Ctrl+D or "Keep this one"; groups that no longer fitted the filter stayed visible until something else refreshed the list.
- Keyboard shortcuts no longer reach the list behind an open dialog. Before, pressing Enter while the confirmation was showing opened the highlighted file, and Space or Ctrl+A changed the selection behind it.

## [1.0.0] - 2026-11-16

First release.

- Nine finders: exact duplicates (size, partial hash, full BLAKE3 or XXH3 hash, optional byte-by-byte check), similar pictures, similar music by tags, same name, empty files, empty folders, broken shortcuts, biggest files and temporary files.
- Keep-rules (newest, oldest, largest, smallest, highest resolution, best audio, shortest or longest path, folder order), reference folders, and the rule that one file of every group always stays.
- Actions: Recycle Bin, move to a folder with undo, replace with hard or symbolic links, delete permanently; every file is checked again right before it is touched.
- Side-by-side picture compare, fingerprint cache for fast rescans, history, export to CSV, HTML and JSON, command line scan, demo mode, dark and light themes.
- Setup program and portable ZIP.

[1.1.0]: https://github.com/zheerkhalid/twinforge/releases/tag/v1.1.0
[1.0.0]: https://github.com/zheerkhalid/twinforge/releases/tag/v1.0.0
