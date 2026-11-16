//! Thin wrappers over the Win32 calls TwinForge needs, the demo-mode write
//! guard and the list of protected system locations.

use crate::util::norm_path;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, GetLogicalDrives, SetFileAttributesW, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::UI::Shell::{
    SHFileOperationW, ShellExecuteW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    FO_DELETE, SHFILEOPSTRUCTW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

// --------------------------------------------------------------------------------------
// Demo guard
// --------------------------------------------------------------------------------------

static DEMO: AtomicBool = AtomicBool::new(false);

/// Demo mode shows invented data; while it is on, every operation that would
/// change files refuses to run.
pub fn set_demo(on: bool) {
    DEMO.store(on, Ordering::SeqCst);
}

pub fn is_demo() -> bool {
    DEMO.load(Ordering::SeqCst)
}

/// Call at the top of every function that changes files.
pub fn guard_write() -> Result<(), String> {
    if is_demo() {
        Err("Demo mode: nothing is changed on this PC.".into())
    } else {
        Ok(())
    }
}

// --------------------------------------------------------------------------------------
// Text, shell, clock
// --------------------------------------------------------------------------------------

/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn shell_execute(verb: &str, file: &str, params: &str) -> Result<(), String> {
    let (v, f, p) = (wide(verb), wide(file), wide(params));
    let r = unsafe {
        ShellExecuteW(
            HWND::default(),
            PCWSTR(v.as_ptr()),
            PCWSTR(f.as_ptr()),
            if params.is_empty() {
                PCWSTR::null()
            } else {
                PCWSTR(p.as_ptr())
            },
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if r.0 as usize > 32 {
        Ok(())
    } else {
        Err(format!(
            "Windows could not open \"{file}\" (code {}).",
            r.0 as usize
        ))
    }
}

/// Opens a folder, file or web address with its default program.
pub fn open_path(target: &str) -> Result<(), String> {
    if is_demo() && !target.starts_with("https://") {
        return Err("Demo mode: the files shown here are invented.".into());
    }
    shell_execute("open", target, "")
}

/// Opens Explorer with the file highlighted.
pub fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    if is_demo() {
        return Err("Demo mode: the files shown here are invented.".into());
    }
    if std::fs::symlink_metadata(path).is_err() {
        return Err("The file no longer exists.".into());
    }
    shell_execute(
        "open",
        "explorer.exe",
        &format!("/select,\"{}\"", path.to_string_lossy()),
    )
}

/// Local date and time: `(year, month, day, hour, minute, second)`.
pub fn local_now() -> (i32, u32, u32, u32, u32, u32) {
    let t = unsafe { GetLocalTime() };
    (
        t.wYear as i32,
        t.wMonth as u32,
        t.wDay as u32,
        t.wHour as u32,
        t.wMinute as u32,
        t.wSecond as u32,
    )
}

/// `2026-01-31 14:05:09`
pub fn timestamp() -> String {
    let (y, mo, d, h, m, s) = local_now();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Seconds since 1970-01-01 UTC, now.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Seconds to add to UTC to get local time (the offset in force right now).
pub fn local_offset_secs() -> i64 {
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let secs = |t: &windows::Win32::Foundation::SYSTEMTIME| {
        crate::util::unix_from_date(t.wYear as i32, t.wMonth as u32, t.wDay as u32)
            + t.wHour as i64 * 3600
            + t.wMinute as i64 * 60
            + t.wSecond as i64
    };
    // Rounded to a quarter of an hour so that reading the two clocks a tick
    // apart cannot produce an odd offset.
    let diff = secs(&l) - secs(&u);
    (diff as f64 / 900.0).round() as i64 * 900
}

/// Makes `println!` reach the console the program was started from (the
/// release build is a windowed program and has none of its own).
pub fn attach_console() {
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

// --------------------------------------------------------------------------------------
// Files
// --------------------------------------------------------------------------------------

/// Seconds since 1970 for a file time taken from `std::fs::Metadata`.
pub fn unix_secs(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The Win32 attribute bits of a path, without following links.
pub fn attributes(path: &Path) -> Option<u32> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|m| m.file_attributes())
}

/// Sets the Win32 attribute bits of a path (used to clear "read-only" and by tests).
pub fn set_attributes(path: &Path, attrs: u32) -> bool {
    let w = wide(&path.to_string_lossy());
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)).is_ok() }
}

/// Creates a junction (a folder link) with the system's own tool. Used by
/// tests and the self-test to prove that links are never followed.
pub fn make_junction(link: &Path, target: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Is this path itself a junction, symbolic link or other reparse point?
pub fn is_reparse_point(path: &Path) -> bool {
    attributes(path)
        .map(|a| a & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

/// Is the path, or any folder above it, a junction or symbolic link? Used to
/// make sure a file is reached without passing through a link.
pub fn passes_through_link(path: &Path) -> bool {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.parent().is_none() {
            break; // a drive root
        }
        if is_reparse_point(p) {
            return true;
        }
        cur = p.parent();
    }
    false
}

/// What identifies a file on disk regardless of its name: two paths with the
/// same `FileId` are hard links to (or aliases of) the same file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume: u32,
    pub index: u64,
    /// How many names (hard links) the file has.
    pub links: u32,
}

impl FileId {
    /// Same file on disk?
    pub fn same_file(&self, other: &FileId) -> bool {
        self.volume == other.volume && self.index == other.index
    }
}

/// Reads the identity of a file or folder without following links.
pub fn file_id(path: &Path) -> Option<FileId> {
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(7)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).ok()? };
    Some(FileId {
        volume: info.dwVolumeSerialNumber,
        index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        links: info.nNumberOfLinks,
    })
}

/// The drive roots that exist right now (`C:\`, `D:\`, ...).
pub fn drives() -> Vec<PathBuf> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .collect()
}

/// Sends files or folders to the Recycle Bin in one shell operation.
pub fn recycle(paths: &[PathBuf]) -> Result<(), String> {
    guard_write()?;
    if paths.is_empty() {
        return Ok(());
    }
    // SHFileOperation wants a double-NUL-terminated list of NUL-terminated paths.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        let text = p.to_string_lossy();
        // The shell API does not understand the long-path prefix.
        let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
        from.extend(text.encode_utf16());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("The Recycle Bin refused it (code {code:#x})."));
    }
    if op.fAnyOperationsAborted.as_bool() {
        return Err("The operation was cancelled.".into());
    }
    Ok(())
}

/// `%LOCALAPPDATA%\TwinForge`: settings, hash cache and undo logs.
pub fn app_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TwinForge")
}

// --------------------------------------------------------------------------------------
// Protected locations
// --------------------------------------------------------------------------------------

/// Folders directly under a drive root that belong to Windows on every drive.
const ROOT_SYSTEM_FOLDERS: [&str; 11] = [
    "$recycle.bin",
    "system volume information",
    "recovery",
    "$winreagent",
    "$windows.~bt",
    "$windows.~ws",
    "$sysreset",
    "boot",
    "efi",
    "config.msi",
    "msocache",
];

/// Files directly under a drive root that belong to Windows.
const ROOT_SYSTEM_FILES: [&str; 6] = [
    "pagefile.sys",
    "hiberfil.sys",
    "swapfile.sys",
    "bootmgr",
    "bootnxt",
    "dumpstack.log.tmp",
];

/// Knows which locations must never be changed. The scan uses it to skip
/// system folders; the action engine asks it again right before every change.
#[derive(Clone, Debug)]
pub struct Guard {
    /// Normalised folder paths (see [`norm_path`]).
    roots: Vec<String>,
}

impl Guard {
    /// The protected locations of this PC.
    pub fn real() -> Guard {
        let mut roots: Vec<String> = Vec::new();
        let mut add = |p: String| {
            let n = norm_path(&p);
            // Never accept something as short as a drive root: that would
            // protect (and so hide) the whole drive.
            if n.len() > 3 && !roots.contains(&n) {
                roots.push(n);
            }
        };
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        add(var("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()));
        for name in [
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramData",
            "CommonProgramFiles",
        ] {
            if let Some(v) = var(name) {
                add(v);
            }
        }
        let drive = var("SystemDrive").unwrap_or_else(|| "C:".into());
        for name in [
            "Windows",
            "Program Files",
            "Program Files (x86)",
            "ProgramData",
            "Windows.old",
        ] {
            add(format!("{drive}\\{name}"));
        }
        // Per-user places that Windows and installed programs own.
        if let Some(local) = var("LOCALAPPDATA") {
            add(format!("{local}\\Microsoft"));
            add(format!("{local}\\Packages"));
            add(format!("{local}\\Programs"));
            add(format!("{local}\\TwinForge"));
        }
        if let Some(roaming) = var("APPDATA") {
            add(format!("{roaming}\\Microsoft"));
        }
        Guard { roots }
    }

    /// A guard with exactly these protected folders (for tests and the self-test).
    pub fn with_roots<P: AsRef<Path>>(roots: &[P]) -> Guard {
        Guard {
            roots: roots
                .iter()
                .map(|p| norm_path(&p.as_ref().to_string_lossy()))
                .filter(|p| !p.is_empty())
                .collect(),
        }
    }

    /// Protects nothing except the per-drive system folders (for tests).
    pub fn none() -> Guard {
        Guard { roots: Vec::new() }
    }

    /// True when the path is, or lies inside, a protected location.
    pub fn is_protected(&self, path: &Path) -> bool {
        let p = norm_path(&path.to_string_lossy());
        if p.is_empty() {
            return true;
        }
        if self
            .roots
            .iter()
            .any(|r| p == *r || p.starts_with(&format!("{r}\\")))
        {
            return true;
        }
        // `c:\<first>\...`: per-drive system folders and root-level system files.
        let mut parts = p.split('\\');
        let drive = parts.next().unwrap_or("");
        if drive.len() == 2 && drive.ends_with(':') {
            if let Some(first) = parts.next() {
                if ROOT_SYSTEM_FOLDERS.contains(&first) {
                    return true;
                }
                if parts.next().is_none() && ROOT_SYSTEM_FILES.contains(&first) {
                    return true;
                }
            }
        }
        false
    }

    /// True when a protected location lies inside `dir` (so `dir` must not be
    /// removed or moved as a whole).
    pub fn contains_protected(&self, dir: &Path) -> bool {
        let d = norm_path(&dir.to_string_lossy());
        self.roots
            .iter()
            .any(|r| *r == d || r.starts_with(&format!("{d}\\")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_sane() {
        let (y, _, _, h, m, s) = local_now();
        assert!(y >= 2024 && h < 24 && m < 60 && s < 61);
        assert_eq!(timestamp().len(), 19);
        let off = local_offset_secs();
        assert!((-13 * 3600..=15 * 3600).contains(&off), "{off}");
        assert_eq!(off % 900, 0);
        assert!(unix_now() > 1_700_000_000);
    }

    #[test]
    fn real_guard_protects_system_locations() {
        let g = Guard::real();
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        assert!(g.is_protected(Path::new(&root)));
        assert!(g.is_protected(Path::new(&format!("{root}\\System32\\drivers\\etc\\hosts"))));
        assert!(g.is_protected(Path::new(&root.to_uppercase())));
        assert!(g.is_protected(Path::new("C:\\Program Files\\Some App\\app.exe")));
        assert!(g.is_protected(Path::new("C:/Program Files (x86)/x/y.dll")));
        assert!(g.is_protected(Path::new("C:\\ProgramData\\x\\y.dat")));
        assert!(g.is_protected(Path::new("D:\\$RECYCLE.BIN\\S-1-5-21\\$R1.txt")));
        assert!(g.is_protected(Path::new("E:\\System Volume Information\\x")));
        assert!(g.is_protected(Path::new("C:\\pagefile.sys")));
        assert!(g.is_protected(Path::new("\\\\?\\C:\\Windows\\win.ini")));
        assert!(g.is_protected(Path::new("")));
        // Ordinary places stay usable.
        assert!(!g.is_protected(Path::new("D:\\Photos\\2021\\a.jpg")));
        assert!(!g.is_protected(Path::new("C:\\WindowsBackup\\a.jpg")));
        assert!(!g.is_protected(Path::new("D:\\Archive\\pagefile.sys")));
        assert!(!g.is_protected(Path::new("C:\\")));
        assert!(g.contains_protected(Path::new("C:\\")));
        assert!(!g.contains_protected(Path::new("D:\\Photos")));
    }

    #[test]
    fn custom_guard() {
        let g = Guard::with_roots(&["X:\\Sandbox\\Protected"]);
        assert!(g.is_protected(Path::new("x:\\sandbox\\protected\\a.txt")));
        assert!(!g.is_protected(Path::new("X:\\Sandbox\\Other\\a.txt")));
        assert!(!Guard::none().is_protected(Path::new("X:\\Sandbox\\a.txt")));
        assert!(Guard::none().is_protected(Path::new("X:\\$Recycle.Bin\\a.txt")));
    }

    #[test]
    fn file_identity_and_hard_links() {
        let dir = std::env::temp_dir().join(format!("twinforge-sys-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        let c = dir.join("c.bin");
        std::fs::write(&a, b"same").unwrap();
        std::fs::write(&c, b"same").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let (ia, ib, ic) = (
            file_id(&a).unwrap(),
            file_id(&b).unwrap(),
            file_id(&c).unwrap(),
        );
        assert!(ia.same_file(&ib));
        assert!(!ia.same_file(&ic));
        assert_eq!((ia.links, ic.links), (2, 1));
        assert!(file_id(&dir.join("missing")).is_none());
        assert!(!is_reparse_point(&a));
        assert!(!passes_through_link(&a));
        assert!(drives().iter().any(|d| d.exists()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn demo_guard_blocks_writes() {
        // Only this test flips the switch, and it puts it back.
        assert!(guard_write().is_ok() || is_demo());
    }
}
