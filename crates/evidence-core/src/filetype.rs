//! Content-based file type detection (declared types and file names are never trusted).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Pdf,
    Xlsx,
    Csv,
    Png,
    Jpeg,
    Tiff,
    /// AutoCAD DXF (ASCII drawing exchange format).
    Dxf,
}

impl FileType {
    pub fn media_type(self) -> &'static str {
        match self {
            FileType::Pdf => "application/pdf",
            FileType::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            FileType::Csv => "text/csv",
            FileType::Png => "image/png",
            FileType::Jpeg => "image/jpeg",
            FileType::Tiff => "image/tiff",
            FileType::Dxf => "image/vnd.dxf",
        }
    }

    pub fn from_media_type(s: &str) -> Option<FileType> {
        [
            FileType::Pdf,
            FileType::Xlsx,
            FileType::Csv,
            FileType::Png,
            FileType::Jpeg,
            FileType::Tiff,
            FileType::Dxf,
        ]
        .into_iter()
        .find(|t| t.media_type() == s)
    }
}

/// ASCII DXF starts with group code 0 followed by SECTION.
fn is_dxf(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(64)]);
    let mut lines = head.lines().map(str::trim).filter(|l| !l.is_empty());
    lines.next() == Some("0") && lines.next() == Some("SECTION")
}

/// Structurally valid Shift_JIS/CP932 text (ASCII, half-width katakana, lead+trail double-byte pairs). Japanese
/// CSV exports are mostly CP932; the extraction worker decodes them (`csv_encoding: shift_jis` import setting).
fn is_shift_jis(b: &[u8]) -> bool {
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            0x00..=0x7F | 0xA1..=0xDF => i += 1,
            0x81..=0x9F | 0xE0..=0xFC if matches!(b.get(i + 1), Some(0x40..=0x7E | 0x80..=0xFC)) => i += 2,
            _ => return false,
        }
    }
    true
}

/// Detects an allowed type from magic bytes. Text is accepted as CSV only if it is valid UTF-8 or Shift_JIS without control characters (except tab/CR/LF).
pub fn detect(b: &[u8]) -> Option<FileType> {
    if b.starts_with(b"%PDF-") {
        Some(FileType::Pdf)
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(FileType::Png)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(FileType::Jpeg)
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Some(FileType::Tiff)
    } else if b.starts_with(b"PK\x03\x04") {
        Some(FileType::Xlsx) // confirmed (or rejected) by the structural scan
    } else if !b.is_empty()
        && !b
            .iter()
            .any(|&c| c < 0x20 && !matches!(c, b'\t' | b'\n' | b'\r') || c == 0x7f)
        && (std::str::from_utf8(b).is_ok() || is_shift_jis(b))
    {
        if is_dxf(b) {
            Some(FileType::Dxf)
        } else {
            Some(FileType::Csv)
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_content_only() {
        assert_eq!(detect(b"%PDF-1.7\n"), Some(FileType::Pdf));
        assert_eq!(detect(b"part,rev\nA-1,C\n"), Some(FileType::Csv));
        assert_eq!(detect(b"MZ\x90\0\x03"), None, "executables rejected");
        assert_eq!(detect(b"a,b\0c"), None);
        assert_eq!(detect(&[0xFD, 0x28]), None, "neither UTF-8 nor Shift_JIS");
        // "品番,材質\nBRK-1,鉄\n" and half-width "ﾊﾟｰﾂ" in CP932: Japanese CSV exports must not be refused at upload.
        assert_eq!(
            detect(b"\x95i\x94\xd4,\x8d\xde\x8e\xbf\nBRK-1,\x93S\n"),
            Some(FileType::Csv)
        );
        assert_eq!(detect(b"\xca\xdf\xb0\xc2\n"), Some(FileType::Csv));
        assert_eq!(detect(b"a,\x95"), None, "truncated double-byte character");
        assert_eq!(detect(b"a,\x95\x0a"), None, "invalid trail byte");
        assert_eq!(detect(b""), None);
        assert_eq!(FileType::from_media_type("text/csv"), Some(FileType::Csv));
        assert_eq!(detect(b"  0\r\nSECTION\r\n  2\r\nHEADER\r\n"), Some(FileType::Dxf));
        assert_eq!(FileType::from_media_type("image/vnd.dxf"), Some(FileType::Dxf));
    }
}
