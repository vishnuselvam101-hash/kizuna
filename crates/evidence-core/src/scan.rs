//! Static upload scanning: structural checks that need no parser execution. This is not antivirus;
//! a real malware scanner can be plugged in later (see ADR 0011). Verdicts are conservative.

use serde::Serialize;

use crate::filetype::{FileType, detect};
use crate::zip::{ZipError, central_directory};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// No finding: may be parsed by the sandboxed worker.
    Clean,
    /// Suspicious content: kept but not parsed or served until a human releases it.
    Quarantine,
    /// Not an allowed or well-formed type: refused.
    Reject,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub verdict: Verdict,
    pub file_type: Option<&'static str>,
    pub findings: Vec<String>,
    pub scanner: &'static str,
}

pub const SCANNER: &str = "kizuna-static-scan@1";

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_zip_entries: usize,
    pub max_zip_uncompressed: u64,
    pub max_zip_ratio: u64,
    pub max_csv_lines: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_zip_entries: 2_000,
            max_zip_uncompressed: 200 * 1024 * 1024,
            max_zip_ratio: 100,
            max_csv_lines: 200_000,
        }
    }
}

const PDF_ACTIVE: [&[u8]; 10] = [
    b"/JavaScript",
    b"/JS",
    b"/Launch",
    b"/EmbeddedFile",
    b"/RichMedia",
    b"/XFA",
    b"/SubmitForm",
    b"/OpenAction",
    b"/AA",
    b"/ObjStm",
];

/// Decodes PDF name escapes (`/J#61vaScript` → `/JavaScript`) so markers cannot be hidden (review finding M5).
fn decode_pdf_names(b: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn contains(h: &[u8], n: &[u8]) -> bool {
    h.windows(n.len()).any(|w| w == n)
}

pub fn scan(bytes: &[u8], limits: &Limits) -> ScanReport {
    let mut findings = Vec::new();
    let Some(ft) = detect(bytes) else {
        return ScanReport {
            verdict: Verdict::Reject,
            file_type: None,
            findings: vec!["type_not_allowed".into()],
            scanner: SCANNER,
        };
    };
    let mut verdict = Verdict::Clean;
    match ft {
        FileType::Pdf => {
            // Compressed object streams (/ObjStm) can hide any marker from a byte scan, so they are quarantined
            // too until a human releases the document (review finding M5).
            let decoded = decode_pdf_names(bytes);
            for k in PDF_ACTIVE {
                if contains(&decoded, k) {
                    findings.push(format!("pdf_active_content:{}", String::from_utf8_lossy(&k[1..])));
                }
            }
            if !findings.is_empty() {
                verdict = Verdict::Quarantine;
            }
            if !contains(&bytes[bytes.len().saturating_sub(2048)..], b"%%EOF") {
                findings.push("pdf_no_eof_marker".into());
            }
        }
        FileType::Xlsx => match central_directory(bytes, limits.max_zip_entries) {
            Err(ZipError::TooManyEntries) => {
                findings.push("zip_too_many_entries".into());
                verdict = Verdict::Quarantine;
            }
            Err(ZipError::Zip64Unsupported) => {
                findings.push("zip64_not_supported".into());
                verdict = Verdict::Quarantine;
            }
            Err(e) => {
                findings.push(format!("zip_malformed:{e:?}"));
                verdict = Verdict::Reject;
            }
            Ok(entries) => {
                let total: u64 = entries.iter().map(|e| e.uncompressed).sum();
                let comp: u64 = entries.iter().map(|e| e.compressed).sum::<u64>().max(1);
                if !entries.iter().any(|e| e.name == "[Content_Types].xml")
                    || !entries.iter().any(|e| e.name.starts_with("xl/"))
                {
                    findings.push("zip_not_xlsx".into());
                    verdict = Verdict::Reject;
                }
                if entries
                    .iter()
                    .any(|e| e.name.contains("..") || e.name.starts_with('/') || e.name.contains('\\'))
                {
                    findings.push("zip_path_traversal_name".into());
                    verdict = Verdict::Quarantine.max_with(verdict);
                }
                if entries.iter().any(|e| e.name.ends_with("vbaProject.bin")) {
                    findings.push("xlsx_macros".into());
                    verdict = Verdict::Quarantine.max_with(verdict);
                }
                if entries.iter().any(|e| e.name.starts_with("xl/externalLinks/")) {
                    findings.push("xlsx_external_links".into());
                    verdict = Verdict::Quarantine.max_with(verdict);
                }
                if total > limits.max_zip_uncompressed || total / comp > limits.max_zip_ratio {
                    findings.push("zip_bomb_suspected".into());
                    verdict = Verdict::Quarantine.max_with(verdict);
                }
            }
        },
        FileType::Csv => {
            if bytes.iter().filter(|&&c| c == b'\n').count() > limits.max_csv_lines {
                findings.push("csv_too_many_lines".into());
                verdict = Verdict::Quarantine;
            }
            // Spreadsheet formula injection markers are recorded; they are inert because KIZUNA never evaluates them,
            // but exports must escape them.
            if bytes.split(|&c| c == b'\n').any(|l| {
                l.split(|&c| c == b',')
                    .any(|f| matches!(f.first(), Some(b'=' | b'+' | b'@')))
            }) {
                findings.push("csv_formula_prefix".into());
            }
        }
        FileType::Png | FileType::Jpeg | FileType::Tiff | FileType::Dxf => {}
    }
    ScanReport {
        verdict,
        file_type: Some(ft.media_type()),
        findings,
        scanner: SCANNER,
    }
}

impl Verdict {
    fn rank(self) -> u8 {
        match self {
            Verdict::Clean => 0,
            Verdict::Quarantine => 1,
            Verdict::Reject => 2,
        }
    }
    fn max_with(self, other: Verdict) -> Verdict {
        if other.rank() > self.rank() { other } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Builds a stored (uncompressed) ZIP with the given (name, declared_uncompressed, data) entries.
    pub fn make_zip(entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, unc, data) in entries {
            let off = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[0u8; 14]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&unc.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            cd.extend_from_slice(&[0u8; 16]);
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&unc.to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0u8; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn xlsx(extra: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut e: Vec<(&str, u32, &[u8])> =
            vec![("[Content_Types].xml", 5, b"<x/>!"), ("xl/workbook.xml", 5, b"<w/>!")];
        e.extend_from_slice(extra);
        make_zip(&e)
    }

    #[test]
    fn pdf_active_content_quarantined() {
        let r = scan(
            b"%PDF-1.7\n1 0 obj << /OpenAction << /S /JavaScript /JS (app.alert(1)) >> >>\n%%EOF",
            &Limits::default(),
        );
        assert_eq!(r.verdict, Verdict::Quarantine);
        assert!(r.findings.iter().any(|f| f.contains("JavaScript")));
        let r = scan(b"%PDF-1.7\n1 0 obj << /Type /Page >>\n%%EOF", &Limits::default());
        assert_eq!(r.verdict, Verdict::Clean);
    }

    #[test]
    fn regression_review_m5_escaped_names_and_object_streams() {
        let esc = scan(
            b"%PDF-1.7\n1 0 obj << /S /J#61vaScript /J#53 (x) >>\n%%EOF",
            &Limits::default(),
        );
        assert_eq!(esc.verdict, Verdict::Quarantine, "{:?}", esc.findings);
        let objstm = scan(
            b"%PDF-1.7\n5 0 obj << /Type /ObjStm /N 3 /Filter /FlateDecode >>\nstream\nxx\nendstream\n%%EOF",
            &Limits::default(),
        );
        assert_eq!(objstm.verdict, Verdict::Quarantine);
        assert_eq!(
            scan(
                b"%PDF-1.7\n1 0 obj << /Type /Page /Name /A#20B >>\n%%EOF",
                &Limits::default()
            )
            .verdict,
            Verdict::Clean
        );
    }

    #[test]
    fn xlsx_checks() {
        assert_eq!(scan(&xlsx(&[]), &Limits::default()).verdict, Verdict::Clean);
        assert_eq!(
            scan(&xlsx(&[("xl/vbaProject.bin", 1, b"x")]), &Limits::default()).verdict,
            Verdict::Quarantine
        );
        assert_eq!(
            scan(&xlsx(&[("../../etc/passwd", 1, b"x")]), &Limits::default()).verdict,
            Verdict::Quarantine
        );
        let bomb = scan(&xlsx(&[("xl/sheet1.xml", 1_000_000_000, b"x")]), &Limits::default());
        assert_eq!(bomb.verdict, Verdict::Quarantine);
        assert!(bomb.findings.contains(&"zip_bomb_suspected".to_string()));
        assert_eq!(
            scan(&make_zip(&[("word/document.xml", 1, b"x")]), &Limits::default()).verdict,
            Verdict::Reject
        );
        assert_eq!(scan(b"PK\x03\x04garbage", &Limits::default()).verdict, Verdict::Reject);
    }

    #[test]
    fn executables_and_binary_rejected() {
        assert_eq!(scan(b"\x7fELF\x02\x01", &Limits::default()).verdict, Verdict::Reject);
        assert_eq!(scan(b"MZ\x90\x00", &Limits::default()).verdict, Verdict::Reject);
    }

    proptest! {
        #[test]
        fn scanner_never_panics(mut data in proptest::collection::vec(any::<u8>(), 0..4096), pk in any::<bool>()) {
            if pk { data.splice(0..0, *b"PK\x03\x04"); }
            let _ = scan(&data, &Limits::default());
        }

        #[test]
        fn mutated_zip_never_panics(idx in 0usize..200, byte in any::<u8>()) {
            let mut z = xlsx(&[("xl/sheet1.xml", 3, b"abc")]);
            let i = idx % z.len();
            z[i] = byte;
            let _ = scan(&z, &Limits::default());
        }
    }
}
