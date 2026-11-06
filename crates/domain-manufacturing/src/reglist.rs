//! Regulation-list versions as dependency nodes (`mfg-reglist@1`, research I6).
//!
//! A tenant imports a versioned substance list (an SVHC-candidate-list-style CSV: substance name, CAS/EC number,
//! date added) as an artifact. Supplier compliance declarations (RoHS/REACH style) record which list version they
//! answered; material composition (FMD-style CSV) links parts to materials and substances. When a new version adds
//! substances, a declaration is stale only if a material it covers could contain one of them — by the supplier's
//! own composition data or the tenant-curated material→substance profile. A material nobody has described is never
//! assumed clean: it yields "cannot rule out". KIZUNA ships no list content (licence); everything is tenant data.
//!
//! Pure code: parsing, diffing and the materiality rule. Storage and invalidation live in the API.

use std::collections::{BTreeMap, BTreeSet};

use entity_resolution::safe_normalize;
use serde::{Deserialize, Serialize};

pub const REGLIST_VERSION: &str = "mfg-reglist@1";

/// Most data rows one import accepts.
pub const MAX_ROWS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    NotUtf8,
    Empty,
    TooManyRows,
    MissingColumn(&'static str),
    /// A row is unusable; `line` is 1-based including the header.
    BadRow {
        line: usize,
        code: &'static str,
    },
}

impl ImportError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotUtf8 => "csv_not_utf8",
            Self::Empty => "csv_empty",
            Self::TooManyRows => "csv_too_many_rows",
            Self::MissingColumn(_) => "csv_missing_column",
            Self::BadRow { code, .. } => code,
        }
    }
}

/// Minimal RFC 4180 reader (quoted fields, doubled quotes, CRLF/LF). Blank lines are skipped.
/// CSV rows with their 1-based starting line.
type Rows = Vec<(usize, Vec<String>)>;

fn parse_csv(text: &str) -> Rows {
    let mut rows = Vec::new();
    let (mut row, mut field) = (Vec::new(), String::new());
    let (mut quoted, mut line, mut start) = (false, 1usize, 1usize);
    let mut it = text.chars().peekable();
    let push_row = |row: &mut Vec<String>, rows: &mut Rows, at: usize| {
        if !(row.len() == 1 && row[0].trim().is_empty()) {
            rows.push((at, std::mem::take(row)));
        }
        row.clear();
    };
    while let Some(c) = it.next() {
        if quoted {
            match c {
                '"' if it.peek() == Some(&'"') => {
                    it.next();
                    field.push('"');
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    field.push(c);
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                push_row(&mut row, &mut rows, start);
                line += 1;
                start = line;
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        push_row(&mut row, &mut rows, start);
    }
    rows
}

fn decode(bytes: &[u8]) -> Result<&str, ImportError> {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(b).map_err(|_| ImportError::NotUtf8)
}

fn norm_header(h: &str) -> String {
    safe_normalize(h).to_lowercase().replace([' ', '_', '-', '.', '/'], "")
}

/// Column index by any of the aliases (EN/JA, case/space/separator-insensitive).
fn col(headers: &[String], aliases: &[&str]) -> Option<usize> {
    let hs: Vec<String> = headers.iter().map(|h| norm_header(h)).collect();
    aliases
        .iter()
        .find_map(|a| hs.iter().position(|h| *h == norm_header(a)))
}

const H_NAME: &[&str] = &["substance", "substance name", "name", "物質名", "物質", "化学物質名"];
const H_CAS: &[&str] = &["cas", "cas no", "cas number", "cas番号", "cas no."];
const H_EC: &[&str] = &["ec", "ec no", "ec number", "ec番号"];
const H_DATE: &[&str] = &[
    "date added",
    "date of inclusion",
    "added",
    "inclusion date",
    "追加日",
    "収載日",
];
const H_PART: &[&str] = &["part", "part no", "part number", "品番", "部品番号"];
const H_MATERIAL: &[&str] = &["material", "homogeneous material", "材料", "材質", "均質材料"];
const H_SUPPLIER: &[&str] = &["supplier", "vendor", "仕入先", "サプライヤー"];
const H_LIST: &[&str] = &["list", "regulation list", "規制リスト", "リスト"];
const H_VERSION: &[&str] = &["list version", "version", "リスト版", "版"];
const H_STATUS: &[&str] = &["status", "declaration", "result", "判定", "宣言"];
const H_MATERIALS: &[&str] = &["materials", "covered materials", "対象材料"];

/// CAS registry number with a valid check digit (e.g. `7439-92-1`).
pub fn valid_cas(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    if p.len() != 3
        || !(2..=7).contains(&p[0].len())
        || p[1].len() != 2
        || p[2].len() != 1
        || !p.iter().all(|x| x.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let digits: Vec<u32> = format!("{}{}", p[0], p[1])
        .bytes()
        .map(|b| u32::from(b - b'0'))
        .collect();
    let sum: u32 = digits.iter().rev().enumerate().map(|(i, d)| (i as u32 + 1) * d).sum();
    sum % 10 == u32::from(p[2].as_bytes()[0] - b'0')
}

/// EC number `NNN-NNN-N` with a valid check digit.
pub fn valid_ec(s: &str) -> bool {
    let d: Vec<u8> = s.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    let shape = s.len() == 9 && s.as_bytes()[3] == b'-' && s.as_bytes()[7] == b'-' && d.len() == 7;
    shape
        && d[..6]
            .iter()
            .enumerate()
            .map(|(i, x)| (i as u32 + 1) * u32::from(*x))
            .sum::<u32>()
            % 11
            == u32::from(d[6])
}

fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stable identity of a substance: CAS when present, else EC, else the normalised name (group entries such as
/// "lead compounds" have no CAS).
pub fn substance_key(name: &str, cas: Option<&str>, ec: Option<&str>) -> Option<String> {
    if let Some(c) = cas.filter(|c| !c.is_empty()) {
        return Some(format!("cas:{c}"));
    }
    if let Some(e) = ec.filter(|e| !e.is_empty()) {
        return Some(format!("ec:{e}"));
    }
    let n = safe_normalize(name);
    (!n.is_empty()).then(|| format!("name:{n}"))
}

/// A material key: the same normalisation as part numbers (full-width → ASCII, upper case, collapsed spaces).
pub fn material_key(s: &str) -> String {
    safe_normalize(&clean(s))
}

fn opt(row: &[String], i: Option<usize>) -> Option<String> {
    i.and_then(|i| row.get(i))
        .map(|s| clean(&safe_normalize_digits(s)))
        .filter(|s| !s.is_empty() && s != "-" && s != "—")
}

/// Full-width digits and hyphens to ASCII (Japanese exports), nothing else changed.
fn safe_normalize_digits(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '０'..='９' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '－' | '‐' | '−' => '-',
            _ => c,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    pub key: String,
    pub name: String,
    pub cas: Option<String>,
    pub ec: Option<String>,
    pub date_added: Option<String>,
}

fn rows_of(bytes: &[u8]) -> Result<(Vec<String>, Rows), ImportError> {
    let mut rows = parse_csv(decode(bytes)?).into_iter();
    let (_, headers) = rows.next().ok_or(ImportError::Empty)?;
    let data: Vec<_> = rows.collect();
    if data.is_empty() {
        return Err(ImportError::Empty);
    }
    if data.len() > MAX_ROWS {
        return Err(ImportError::TooManyRows);
    }
    Ok((headers, data))
}

/// Parses a substance list. Each row needs a name; CAS and EC numbers, when given, must carry valid check digits.
/// Duplicate substances (same key) are an error: a list version is a set.
pub fn parse_list(bytes: &[u8]) -> Result<Vec<ListEntry>, ImportError> {
    let (h, data) = rows_of(bytes)?;
    let name = col(&h, H_NAME).ok_or(ImportError::MissingColumn("substance"))?;
    let (cas, ec, date) = (col(&h, H_CAS), col(&h, H_EC), col(&h, H_DATE));
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(data.len());
    for (line, r) in data {
        let n = opt(&r, Some(name)).ok_or(ImportError::BadRow {
            line,
            code: "reglist_missing_name",
        })?;
        let c = opt(&r, cas);
        let e = opt(&r, ec);
        if c.as_deref().is_some_and(|c| !valid_cas(c)) {
            return Err(ImportError::BadRow {
                line,
                code: "reglist_invalid_cas",
            });
        }
        if e.as_deref().is_some_and(|e| !valid_ec(e)) {
            return Err(ImportError::BadRow {
                line,
                code: "reglist_invalid_ec",
            });
        }
        let key = substance_key(&n, c.as_deref(), e.as_deref()).ok_or(ImportError::BadRow {
            line,
            code: "reglist_missing_name",
        })?;
        if !seen.insert(key.clone()) {
            return Err(ImportError::BadRow {
                line,
                code: "reglist_duplicate_substance",
            });
        }
        out.push(ListEntry {
            key,
            name: n.chars().take(500).collect(),
            cas: c,
            ec: e,
            date_added: opt(&r, date).map(|d| d.chars().take(40).collect()),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Diff {
    pub added: Vec<ListEntry>,
    pub removed: Vec<ListEntry>,
    pub unchanged: usize,
}

/// Set difference by substance key (entries sorted by key).
pub fn diff(old: &[ListEntry], new: &[ListEntry]) -> Diff {
    let o: BTreeMap<&str, &ListEntry> = old.iter().map(|e| (e.key.as_str(), e)).collect();
    let n: BTreeMap<&str, &ListEntry> = new.iter().map(|e| (e.key.as_str(), e)).collect();
    Diff {
        added: n
            .iter()
            .filter(|(k, _)| !o.contains_key(*k))
            .map(|(_, e)| (*e).clone())
            .collect(),
        removed: o
            .iter()
            .filter(|(k, _)| !n.contains_key(*k))
            .map(|(_, e)| (*e).clone())
            .collect(),
        unchanged: n.keys().filter(|k| o.contains_key(*k)).count(),
    }
}

/// One FMD row: part → homogeneous material (→ substance when declared).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FmdRow {
    pub line: usize,
    pub part: String,
    pub material: String,
    pub substance: Option<String>,
    pub substance_name: Option<String>,
    pub supplier: Option<String>,
}

/// Parses an FMD-like composition CSV (part, material, optional substance name/CAS, optional supplier).
pub fn parse_fmd(bytes: &[u8]) -> Result<Vec<FmdRow>, ImportError> {
    let (h, data) = rows_of(bytes)?;
    let part = col(&h, H_PART).ok_or(ImportError::MissingColumn("part"))?;
    let material = col(&h, H_MATERIAL).ok_or(ImportError::MissingColumn("material"))?;
    let (name, cas, ec, supplier) = (col(&h, H_NAME), col(&h, H_CAS), col(&h, H_EC), col(&h, H_SUPPLIER));
    let mut out = Vec::with_capacity(data.len());
    for (line, r) in data {
        let p = opt(&r, Some(part)).ok_or(ImportError::BadRow {
            line,
            code: "fmd_missing_part",
        })?;
        let m = opt(&r, Some(material)).ok_or(ImportError::BadRow {
            line,
            code: "fmd_missing_material",
        })?;
        let (n, c, e) = (opt(&r, name), opt(&r, cas), opt(&r, ec));
        if c.as_deref().is_some_and(|c| !valid_cas(c)) {
            return Err(ImportError::BadRow {
                line,
                code: "reglist_invalid_cas",
            });
        }
        if e.as_deref().is_some_and(|e| !valid_ec(e)) {
            return Err(ImportError::BadRow {
                line,
                code: "reglist_invalid_ec",
            });
        }
        let substance = substance_key(n.as_deref().unwrap_or(""), c.as_deref(), e.as_deref());
        out.push(FmdRow {
            line,
            part: safe_normalize(&p),
            material: material_key(&m),
            substance,
            substance_name: n,
            supplier: opt(&r, supplier),
        });
    }
    Ok(out)
}

/// What a supplier declared for one part against one list version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclStatus {
    /// Contains no listed substance above threshold.
    Compliant,
    /// Contains a listed substance (disclosed).
    ContainsListed,
}

impl DeclStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Compliant => "compliant",
            Self::ContainsListed => "contains_listed",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match norm_header(s).as_str() {
            "compliant" | "free" | "notcontained" | "適合" | "非含有" | "不含有" => Some(Self::Compliant),
            "contains" | "containslisted" | "contained" | "含有" => Some(Self::ContainsListed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclRow {
    pub line: usize,
    pub supplier: String,
    pub part: String,
    pub list: String,
    pub version: String,
    pub status: DeclStatus,
    /// Materials the declaration covers; empty = the whole part.
    pub materials: Vec<String>,
}

/// List codes are tenant identifiers such as `svhc` or `rohs`.
pub fn valid_list_code(s: &str) -> bool {
    (2..=40).contains(&s.len())
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Version labels: printable, short (e.g. `2026-01`, `v35`).
pub fn valid_version_label(s: &str) -> bool {
    (1..=40).contains(&s.chars().count()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// Parses a compliance-declaration CSV (supplier, part, list, list version, status, optional covered materials
/// separated by `;`).
pub fn parse_declarations(bytes: &[u8]) -> Result<Vec<DeclRow>, ImportError> {
    let (h, data) = rows_of(bytes)?;
    let supplier = col(&h, H_SUPPLIER).ok_or(ImportError::MissingColumn("supplier"))?;
    let part = col(&h, H_PART).ok_or(ImportError::MissingColumn("part"))?;
    let list = col(&h, H_LIST).ok_or(ImportError::MissingColumn("list"))?;
    let version = col(&h, H_VERSION).ok_or(ImportError::MissingColumn("list_version"))?;
    let status = col(&h, H_STATUS).ok_or(ImportError::MissingColumn("status"))?;
    let materials = col(&h, H_MATERIALS);
    let mut out = Vec::with_capacity(data.len());
    for (line, r) in data {
        let bad = |code| ImportError::BadRow { line, code };
        let s = opt(&r, Some(supplier)).ok_or(bad("decl_missing_supplier"))?;
        let p = opt(&r, Some(part)).ok_or(bad("decl_missing_part"))?;
        let l = opt(&r, Some(list))
            .map(|l| l.to_lowercase())
            .filter(|l| valid_list_code(l));
        let v = opt(&r, Some(version)).filter(|v| valid_version_label(v));
        let st = opt(&r, Some(status)).and_then(|x| DeclStatus::parse(&x));
        let mats: BTreeSet<String> = opt(&r, materials)
            .map(|m| m.split(';').map(material_key).filter(|m| !m.is_empty()).collect())
            .unwrap_or_default();
        out.push(DeclRow {
            line,
            supplier: s.chars().take(200).collect(),
            part: safe_normalize(&p),
            list: l.ok_or(bad("decl_invalid_list"))?,
            version: v.ok_or(bad("decl_invalid_version"))?,
            status: st.ok_or(bad("decl_invalid_status"))?,
            materials: mats.into_iter().collect(),
        });
    }
    Ok(out)
}

/// Why a declaration is (not) flagged by a list change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// A covered material could contain an added substance.
    MayContain {
        substances: Vec<String>,
        materials: Vec<String>,
        /// Covered materials nobody has described (also could contain anything).
        unknown_materials: Vec<String>,
    },
    /// Composition unknown (no FMD and no curated profile): KIZUNA cannot show the part is unaffected.
    CannotRuleOut {
        reason: &'static str,
        substances: Vec<String>,
        unknown_materials: Vec<String>,
    },
    NotAffected,
}

impl Verdict {
    pub fn flagged(&self) -> bool {
        !matches!(self, Self::NotAffected)
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::MayContain { .. } => "may_contain",
            Self::CannotRuleOut { .. } => "cannot_rule_out",
            Self::NotAffected => "not_affected",
        }
    }
}

/// The materiality rule. `added`: substance keys the new version adds relative to the version the declaration
/// answered. `part_fmd`: the part's composition from FMD claims (material → declared substances; a material with
/// an empty set is known to be present but its substances were not declared). `covered`: the declaration's
/// covered materials (empty = all the part's materials). `profiles`: tenant-curated material → substances it could
/// contain (an entry with an empty set = curated as containing none of interest).
pub fn assess(
    added: &BTreeSet<String>,
    part_fmd: &BTreeMap<String, BTreeSet<String>>,
    covered: &[String],
    profiles: &BTreeMap<String, BTreeSet<String>>,
) -> Verdict {
    if added.is_empty() {
        return Verdict::NotAffected;
    }
    let materials: BTreeSet<&String> = if covered.is_empty() {
        part_fmd.keys().collect()
    } else {
        covered.iter().collect()
    };
    if materials.is_empty() {
        return Verdict::CannotRuleOut {
            reason: "composition_unknown",
            substances: added.iter().cloned().collect(),
            unknown_materials: vec![],
        };
    }
    let (mut hits, mut hit_mats, mut unknown) = (BTreeSet::new(), Vec::new(), Vec::new());
    for m in materials {
        let fmd = part_fmd.get(m).filter(|s| !s.is_empty());
        let prof = profiles.get(m);
        if fmd.is_none() && prof.is_none() {
            unknown.push(m.clone());
            continue;
        }
        let h: Vec<&String> = fmd
            .into_iter()
            .flatten()
            .chain(prof.into_iter().flatten())
            .filter(|s| added.contains(*s))
            .collect();
        if !h.is_empty() {
            hit_mats.push(m.clone());
            hits.extend(h.into_iter().cloned());
        }
    }
    if !hits.is_empty() {
        Verdict::MayContain {
            substances: hits.into_iter().collect(),
            materials: hit_mats,
            unknown_materials: unknown,
        }
    } else if !unknown.is_empty() {
        Verdict::CannotRuleOut {
            reason: "material_unmapped",
            substances: added.iter().cloned().collect(),
            unknown_materials: unknown,
        }
    } else {
        Verdict::NotAffected
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn set(xs: &[&str]) -> BTreeSet<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    // Fictional list: names are invented; the CAS/EC numbers are only shaped to pass the check-digit rules.
    const V1: &str = "Substance name,CAS No.,EC No.,Date added\n\
Kizunium oxide,1234-56-6,,2025-06-01\n\
\"Tetrafoo, bis-\",50-00-0,200-001-8,2025-06-01\n\
Group: imaginary salts,,,2025-06-01\n";

    #[test]
    fn check_digits() {
        assert!(valid_cas("7732-18-5")); // water
        assert!(valid_cas("50-00-0"));
        assert!(valid_cas("1234-56-6"));
        assert!(!valid_cas("1234-56-7"));
        assert!(!valid_cas("12-3-4"));
        assert!(valid_ec("200-001-8"));
        assert!(!valid_ec("200-001-9"));
        assert!(!valid_ec("2000018"));
    }

    #[test]
    fn parses_list_with_quotes_bom_and_group_entries() {
        let mut b = b"\xEF\xBB\xBF".to_vec();
        b.extend_from_slice(V1.as_bytes());
        let l = parse_list(&b).unwrap();
        assert_eq!(l.len(), 3);
        assert_eq!(l[1].name, "Tetrafoo, bis-");
        assert_eq!(l[1].key, "cas:50-00-0");
        assert_eq!(l[2].key, "name:GROUP: IMAGINARY SALTS");
        assert_eq!(l[0].date_added.as_deref(), Some("2025-06-01"));
    }

    #[test]
    fn japanese_headers_and_full_width_digits() {
        let csv = "物質名,CAS番号,追加日\n架空酸化物,１２３４－５６－６,2025-06-01\n";
        let l = parse_list(csv.as_bytes()).unwrap();
        assert_eq!(l[0].key, "cas:1234-56-6");
    }

    #[test]
    fn rejects_bad_rows_with_line_numbers() {
        let e = parse_list(b"name,cas\nA,1234-56-7\n").unwrap_err();
        assert_eq!(
            e,
            ImportError::BadRow {
                line: 2,
                code: "reglist_invalid_cas"
            }
        );
        let e = parse_list(b"name,cas\nA,50-00-0\nB,50-00-0\n").unwrap_err();
        assert_eq!(e.code(), "reglist_duplicate_substance");
        assert_eq!(parse_list(b"cas\n50-00-0\n").unwrap_err().code(), "csv_missing_column");
        assert_eq!(parse_list(b"name\n").unwrap_err().code(), "csv_empty");
        assert_eq!(parse_list(b"name\n\xff\n").unwrap_err().code(), "csv_not_utf8");
    }

    #[test]
    fn diff_by_key() {
        let a = parse_list(V1.as_bytes()).unwrap();
        let b = parse_list(b"name,cas\nTetrafoo,50-00-0\nNewium,7732-18-5\n").unwrap();
        let d = diff(&a, &b);
        assert_eq!(
            d.added.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
            ["cas:7732-18-5"]
        );
        assert_eq!(d.removed.len(), 2);
        assert_eq!(d.unchanged, 1);
    }

    #[test]
    fn parses_fmd_and_declarations() {
        let f =
            parse_fmd("品番,材料,物質名,CAS番号\nbrk-1,PBT gf30,Kizunium oxide,1234-56-6\nBRK-1,SUS304,,\n".as_bytes())
                .unwrap();
        assert_eq!(f[0].part, "BRK-1");
        assert_eq!(f[0].material, "PBT GF30");
        assert_eq!(f[0].substance.as_deref(), Some("cas:1234-56-6"));
        assert_eq!(f[1].substance, None);
        let d = parse_declarations(
            b"Supplier,Part,List,List version,Status,Materials\nNagoya Forge,BRK-1,SVHC,2025-06,compliant,pbt gf30; sus304\n",
        )
        .unwrap();
        assert_eq!(d[0].list, "svhc");
        assert_eq!(d[0].materials, ["PBT GF30", "SUS304"]);
        assert_eq!(d[0].status, DeclStatus::Compliant);
        let e = parse_declarations(b"supplier,part,list,version,status\nA,P,svhc,v1,maybe\n").unwrap_err();
        assert_eq!(e.code(), "decl_invalid_status");
    }

    #[test]
    fn materiality_rule() {
        let added = set(&["cas:1", "cas:2"]);
        let profiles: BTreeMap<String, BTreeSet<String>> =
            [("PVC".to_string(), set(&["cas:1"])), ("SUS304".to_string(), set(&[]))].into();
        let fmd = |ms: &[(&str, &[&str])]| -> BTreeMap<String, BTreeSet<String>> {
            ms.iter().map(|(m, s)| (m.to_string(), set(s))).collect()
        };
        // Profile says PVC could contain cas:1.
        assert_eq!(
            assess(&added, &fmd(&[("PVC", &[])]), &[], &profiles).code(),
            "may_contain"
        );
        // Curated clean metal: not affected.
        assert_eq!(
            assess(&added, &fmd(&[("SUS304", &[])]), &[], &profiles),
            Verdict::NotAffected
        );
        // FMD declares the substance itself.
        assert_eq!(
            assess(&added, &fmd(&[("ABS", &["cas:2"])]), &[], &profiles).code(),
            "may_contain"
        );
        // FMD lists other substances only: known, not affected.
        assert_eq!(
            assess(&added, &fmd(&[("ABS", &["cas:9"])]), &[], &profiles),
            Verdict::NotAffected
        );
        // Material present, nothing known about it.
        let v = assess(&added, &fmd(&[("ABS", &[])]), &[], &profiles);
        assert!(matches!(
            v,
            Verdict::CannotRuleOut {
                reason: "material_unmapped",
                ..
            }
        ));
        // No composition at all.
        let v = assess(&added, &BTreeMap::new(), &[], &profiles);
        assert!(matches!(
            v,
            Verdict::CannotRuleOut {
                reason: "composition_unknown",
                ..
            }
        ));
        // Declaration covering only the clean material is not affected by PVC elsewhere in the part.
        let part = fmd(&[("PVC", &[]), ("SUS304", &[])]);
        assert_eq!(
            assess(&added, &part, &["SUS304".into()], &profiles),
            Verdict::NotAffected
        );
        assert!(assess(&added, &part, &[], &profiles).flagged());
        // Nothing added: never flagged.
        assert_eq!(
            assess(&BTreeSet::new(), &BTreeMap::new(), &[], &profiles),
            Verdict::NotAffected
        );
    }

    type Sets = BTreeMap<String, BTreeSet<String>>;

    fn arb_world() -> impl Strategy<Value = (BTreeSet<String>, Sets, Sets)> {
        let sub = (0u8..6).prop_map(|i| format!("cas:{i}"));
        let mat = (0u8..5).prop_map(|i| format!("M{i}"));
        (
            prop::collection::btree_set(sub.clone(), 0..4),
            prop::collection::btree_map(mat.clone(), prop::collection::btree_set(sub.clone(), 0..3), 0..4),
            prop::collection::btree_map(mat, prop::collection::btree_set(sub, 0..3), 0..4),
        )
    }

    proptest! {
        /// Soundness: "not affected" only when every covered material is described and none of its possible
        /// substances was added.
        #[test]
        fn not_affected_is_sound((added, fmd, prof) in arb_world()) {
            if assess(&added, &fmd, &[], &prof) == Verdict::NotAffected && !added.is_empty() {
                prop_assert!(!fmd.is_empty());
                for (m, s) in &fmd {
                    let known = !s.is_empty() || prof.contains_key(m);
                    prop_assert!(known);
                    let possible: BTreeSet<&String> = s.iter().chain(prof.get(m).into_iter().flatten()).collect();
                    prop_assert!(possible.iter().all(|x| !added.contains(*x)));
                }
            }
        }

        /// Flagging is monotone in the added set: adding more substances never clears a flag.
        #[test]
        fn monotone_in_added((added, fmd, prof) in arb_world(), extra in prop::collection::btree_set((0u8..6).prop_map(|i| format!("cas:{i}")), 0..3)) {
            let more: BTreeSet<String> = added.union(&extra).cloned().collect();
            if assess(&added, &fmd, &[], &prof).flagged() {
                prop_assert!(assess(&more, &fmd, &[], &prof).flagged());
            }
        }

        /// Diff identities: added ∩ old = ∅ and (unchanged + added) = |new|.
        #[test]
        fn diff_identities(a in prop::collection::btree_set(0u16..50, 0..20), b in prop::collection::btree_set(0u16..50, 0..20)) {
            let e = |k: &u16| ListEntry { key: format!("k{k}"), name: format!("n{k}"), cas: None, ec: None, date_added: None };
            let (old, new): (Vec<_>, Vec<_>) = (a.iter().map(e).collect(), b.iter().map(e).collect());
            let d = diff(&old, &new);
            prop_assert_eq!(d.added.len() + d.unchanged, new.len());
            prop_assert_eq!(d.removed.len() + d.unchanged, old.len());
            prop_assert!(d.added.iter().all(|x| !old.contains(x)));
        }
    }
}
