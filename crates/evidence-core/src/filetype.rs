//! Content-based file type detection (declared types and file names are never trusted).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Pdf,
    Xlsx,
    Csv,
    Png,
    Jpeg,
    Tiff,
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
        ]
        .into_iter()
        .find(|t| t.media_type() == s)
    }
}

/// Detects an allowed type from magic bytes. Text is accepted as CSV only if it is valid UTF-8 without control characters (except tab/CR/LF).
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
        && std::str::from_utf8(b).is_ok()
    {
        Some(FileType::Csv)
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
        assert_eq!(detect(&[0xC3, 0x28]), None, "invalid UTF-8");
        assert_eq!(detect(b""), None);
        assert_eq!(FileType::from_media_type("text/csv"), Some(FileType::Csv));
    }
}
