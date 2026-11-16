//! A tiny reader for Windows shortcut (.lnk) files: just enough to learn where
//! a shortcut points, without COM. Pure, so it can be unit-tested.

use crate::util::path_within;

const HEADER_SIZE: usize = 0x4C;
const CLSID: [u8; 16] = [
    0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
];

fn u16_at(b: &[u8], off: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?) as usize)
}
fn u32_at(b: &[u8], off: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?) as usize)
}

fn ansi_z(b: &[u8], off: usize) -> Option<String> {
    let rest = b.get(off..)?;
    let end = rest.iter().position(|c| *c == 0)?;
    // Not a full code-page decode, but exact for ASCII paths; others fall back to the raw scan.
    Some(rest[..end].iter().map(|c| *c as char).collect())
}

fn utf16_z(b: &[u8], off: usize) -> Option<String> {
    let rest = b.get(off..)?;
    let mut units = Vec::new();
    for pair in rest.as_chunks::<2>().0 {
        let u = u16::from_le_bytes(*pair);
        if u == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(u);
    }
    None
}

/// The local path a shortcut points at, if it records one.
pub fn target(bytes: &[u8]) -> Option<String> {
    if u32_at(bytes, 0)? != HEADER_SIZE || bytes.get(4..20)? != CLSID {
        return None;
    }
    let flags = u32_at(bytes, 20)?;
    let mut off = HEADER_SIZE;
    if flags & 0x1 != 0 {
        // LinkTargetIDList
        off += 2 + u16_at(bytes, off)?;
    }
    if flags & 0x2 == 0 {
        return None; // no LinkInfo
    }
    let info = bytes.get(off..)?;
    let size = u32_at(info, 0)?;
    let info = info.get(..size)?;
    let header = u32_at(info, 4)?;
    let info_flags = u32_at(info, 8)?;
    if info_flags & 0x1 == 0 {
        return None; // network-only link
    }
    let (base, suffix) = if header >= 0x24 {
        (
            utf16_z(info, u32_at(info, 28)?),
            utf16_z(info, u32_at(info, 32)?),
        )
    } else {
        (
            ansi_z(info, u32_at(info, 16)?),
            ansi_z(info, u32_at(info, 24)?),
        )
    };
    let mut path = base?;
    if let Some(s) = suffix {
        path.push_str(&s);
    }
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle))
}

/// True when the shortcut points into `dir` (target, working folder or icon).
pub fn references(bytes: &[u8], dir: &str) -> bool {
    let dir = dir.trim().trim_end_matches(['\\', '/']);
    if dir.len() < 4 {
        return false;
    }
    if let Some(t) = target(bytes) {
        if path_within(&t, dir) {
            return true;
        }
    }
    // Fallback: the folder followed by a separator appears somewhere in the file.
    let needle = format!("{}\\", dir.replace('/', "\\"));
    if !needle.is_ascii() {
        let wide: Vec<u8> = needle
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        return bytes.windows(wide.len()).any(|w| w == wide.as_slice());
    }
    let wide: Vec<u8> = needle
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    contains_ci(bytes, needle.as_bytes()) || contains_ci(bytes, &wide)
}

/// Builds a minimal valid shortcut pointing at `target_path` (ASCII paths).
/// Used by the self-test and the unit tests to plant realistic shortcuts.
pub fn build(target_path: &str) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    b.extend_from_slice(&CLSID);
    b.extend_from_slice(&0x2u32.to_le_bytes()); // HasLinkInfo
    b.extend_from_slice(&0x20u32.to_le_bytes()); // FILE_ATTRIBUTE_ARCHIVE
    b.resize(HEADER_SIZE - 16, 0); // times, size, icon index
    b.extend_from_slice(&1u32.to_le_bytes()); // SW_SHOWNORMAL
    b.resize(HEADER_SIZE, 0);

    let volume_id: Vec<u8> = {
        let mut v = Vec::new();
        v.extend_from_slice(&0x11u32.to_le_bytes()); // size
        v.extend_from_slice(&3u32.to_le_bytes()); // DRIVE_FIXED
        v.extend_from_slice(&0u32.to_le_bytes()); // serial
        v.extend_from_slice(&0x10u32.to_le_bytes()); // label offset
        v.push(0); // empty label
        v
    };
    let header = 0x1Cusize;
    let base_off = header + volume_id.len();
    let suffix_off = base_off + target_path.len() + 1;
    let size = suffix_off + 1;
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&(header as u32).to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // VolumeIDAndLocalBasePath
    b.extend_from_slice(&(header as u32).to_le_bytes()); // VolumeIDOffset
    b.extend_from_slice(&(base_off as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // no network link
    b.extend_from_slice(&(suffix_off as u32).to_le_bytes());
    b.extend_from_slice(&volume_id);
    b.extend_from_slice(target_path.as_bytes());
    b.push(0);
    b.push(0); // empty common path suffix
    b.extend_from_slice(&0u32.to_le_bytes()); // terminal block
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let bytes = build("C:\\Program Files\\Photo Studio\\app.exe");
        assert_eq!(
            target(&bytes).as_deref(),
            Some("C:\\Program Files\\Photo Studio\\app.exe")
        );
        assert!(references(&bytes, "c:\\program files\\photo studio"));
        assert!(references(&bytes, "C:\\Program Files\\Photo Studio\\"));
        assert!(!references(&bytes, "C:\\Program Files\\Photo"));
        assert!(!references(&bytes, "C:\\Program Files\\Photo Studio Pro"));
        assert!(!references(&bytes, "C:\\"));
    }

    #[test]
    fn garbage_is_rejected() {
        assert_eq!(target(b""), None);
        assert_eq!(target(b"not a shortcut at all, just text that is long enough to pass the header size check......"), None);
        let mut truncated = build("C:\\A\\b.exe");
        truncated.truncate(90);
        assert_eq!(target(&truncated), None);
        assert!(!references(b"", "C:\\Apps\\Foo"));
    }

    #[test]
    fn raw_scan_finds_unicode_strings() {
        // A shortcut whose LinkInfo is missing but which mentions the folder as UTF-16.
        let mut bytes = vec![0u8; 100];
        bytes.extend(
            "D:\\Tools\\My App\\run.exe"
                .encode_utf16()
                .flat_map(|u| u.to_le_bytes()),
        );
        assert!(references(&bytes, "d:\\tools\\my app"));
        assert!(!references(&bytes, "d:\\tools\\my"));
    }
}
