//! Labelled datasets and scoring for the ER v1 vs v2 comparison (`kizuna-er-compare`).
//!
//! The Japanese supplier/part set is synthetic and deterministic by seed: every surface form carries the id of the
//! entity it was generated from, so pairwise ground truth is exact. Names are invented combinations of common
//! place/business words; no real company list is used.

use entity_resolution::v2::{Kind, Rng};
use std::collections::{BTreeMap, BTreeSet};

/// (standard kanji, itaiji variant or same, katakana reading, wāpuro romaji)
const PLACES: &[(&str, &str, &str, &str)] = &[
    ("東海", "東海", "トウカイ", "toukai"),
    ("関東", "關東", "カントウ", "kantou"),
    ("大阪", "大阪", "オオサカ", "oosaka"),
    ("名古屋", "名古屋", "ナゴヤ", "nagoya"),
    ("千葉", "千葉", "チバ", "chiba"),
    ("高橋", "髙橋", "タカハシ", "takahashi"),
    ("山崎", "山﨑", "ヤマザキ", "yamazaki"),
    ("斎藤", "齋藤", "サイトウ", "saitou"),
    ("渡辺", "渡邊", "ワタナベ", "watanabe"),
    ("浜松", "濱松", "ハママツ", "hamamatsu"),
    ("北沢", "北澤", "キタザワ", "kitazawa"),
    ("中央", "中央", "チュウオウ", "chuuou"),
    ("日本", "日本", "ニッポン", "nippon"),
    ("新橋", "新橋", "シンバシ", "shinbashi"),
    ("富士", "富士", "フジ", "fuji"),
    ("三協", "三協", "サンキョウ", "sankyou"),
    ("大東", "大東", "ダイトウ", "daitou"),
    ("京浜", "京濱", "ケイヒン", "keihin"),
    ("東洋", "東洋", "トウヨウ", "touyou"),
    ("昭和", "昭和", "ショウワ", "shouwa"),
    ("太平", "太平", "タイヘイ", "taihei"),
    ("共栄", "共榮", "キョウエイ", "kyouei"),
    ("広島", "廣島", "ヒロシマ", "hiroshima"),
    ("桜井", "櫻井", "サクライ", "sakurai"),
    ("菊池", "菊池", "キクチ", "kikuchi"),
    ("長野", "長野", "ナガノ", "nagano"),
    ("静岡", "靜岡", "シズオカ", "shizuoka"),
    ("岡山", "岡山", "オカヤマ", "okayama"),
    ("神田", "神田", "カンダ", "kanda"),
    ("丸一", "丸一", "マルイチ", "maruichi"),
];

const BUSINESSES: &[(&str, &str, &str, &str)] = &[
    ("精機", "精機", "セイキ", "seiki"),
    ("精密", "精密", "セイミツ", "seimitsu"),
    ("金属", "金屬", "キンゾク", "kinzoku"),
    ("工業", "工業", "コウギョウ", "kougyou"),
    ("興業", "興業", "コウギョウ", "kougyou"),
    ("製作所", "製作所", "セイサクショ", "seisakusho"),
    ("電機", "電機", "デンキ", "denki"),
    ("化成", "化成", "カセイ", "kasei"),
    ("鋳造", "鑄造", "チュウゾウ", "chuuzou"),
    ("発条", "發條", "ハツジョウ", "hatsujou"),
    ("産業", "産業", "サンギョウ", "sangyou"),
    ("工機", "工機", "コウキ", "kouki"),
    ("鉄工", "鐵工", "テッコウ", "tekkou"),
    ("機械", "機械", "キカイ", "kikai"),
    ("技研", "技研", "ギケン", "giken"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Legal {
    Kk,
    Yk,
    Gk,
}

/// One labelled surface form.
#[derive(Debug, Clone)]
pub struct Item {
    pub text: String,
    pub entity: u32,
    /// Which transform produced it (for per-variant recall).
    pub variant: &'static str,
}

#[derive(Debug, Clone)]
pub struct Dataset {
    pub name: String,
    pub kind: Kind,
    pub items: Vec<Item>,
}

impl Dataset {
    /// Distinct surface strings (first occurrence wins; identical strings of two entities would be unlabellable,
    /// so the generators never produce them — checked here).
    pub fn keys(&self) -> Vec<String> {
        let mut seen: BTreeMap<&str, u32> = BTreeMap::new();
        for it in &self.items {
            let e = *seen.entry(&it.text).or_insert(it.entity);
            assert_eq!(e, it.entity, "string {:?} labelled with two entities", it.text);
        }
        seen.keys().map(|s| s.to_string()).collect()
    }

    /// Ground-truth pairs (sorted strings) and the variant label of each pair (the non-canonical side).
    pub fn truth(&self) -> BTreeMap<(String, String), &'static str> {
        let mut by_entity: BTreeMap<u32, BTreeMap<&str, &'static str>> = BTreeMap::new();
        for it in &self.items {
            by_entity
                .entry(it.entity)
                .or_default()
                .entry(&it.text)
                .or_insert(it.variant);
        }
        let mut out = BTreeMap::new();
        for forms in by_entity.values() {
            let v: Vec<(&&str, &&'static str)> = forms.iter().collect();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    let (a, b) = (v[i].0.to_string(), v[j].0.to_string());
                    let label = if *v[i].1 == "canonical" { *v[j].1 } else { *v[i].1 };
                    out.insert(if a < b { (a, b) } else { (b, a) }, label);
                }
            }
        }
        out
    }
}

fn pick<'a, T>(rng: &mut Rng, v: &'a [T]) -> &'a T {
    &v[rng.below(v.len() as u64) as usize]
}

fn hepburn_short(w: &str) -> String {
    w.replace("ou", "o").replace("uu", "u").replace("oo", "o")
}

fn macron(w: &str) -> String {
    w.replace("ou", "ō").replace("uu", "ū").replace("oo", "ō")
}

fn title(w: &str) -> String {
    let mut c = w.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn half_width(kana: &str) -> String {
    const FULL: &str = "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";
    let mut out = String::new();
    for c in kana.chars() {
        let (base, mark) = match c {
            'ガ'..='ド' | 'バ'..='ポ' => {
                let n = c as u32;
                let voiced = "ガギグゲゴザジズゼゾダヂヅデドバビブベボ".contains(c);
                if voiced {
                    (char::from_u32(n - 1).unwrap_or(c), Some('ﾞ'))
                } else if "パピプペポ".contains(c) {
                    (char::from_u32(n - 2).unwrap_or(c), Some('ﾟ'))
                } else {
                    (c, None)
                }
            }
            _ => (c, None),
        };
        match FULL.chars().position(|x| x == base) {
            Some(i) => out.push(char::from_u32(0xFF66 + i as u32).unwrap_or(base)),
            None => out.push(base),
        }
        if let Some(m) = mark {
            out.push(m);
        }
    }
    out
}

fn enlarge_small(kana: &str) -> String {
    kana.chars()
        .map(|c| match c {
            'ャ' => 'ヤ',
            'ュ' => 'ユ',
            'ョ' => 'ヨ',
            'ッ' => 'ツ',
            _ => c,
        })
        .collect()
}

fn long_vowel_bar(kana: &str) -> String {
    // トウカイ → トーカイ, ョウ → ョー (common informal spelling).
    let mut out = String::new();
    let mut prev: Option<char> = None;
    for c in kana.chars() {
        if c == 'ウ' && prev.is_some_and(|p| "オコソトノホモヨロゴゾドボポョ".contains(p)) {
            out.push('ー');
        } else {
            out.push(c);
        }
        prev = Some(c);
    }
    out
}

fn ja_legal(l: Legal, style: u64) -> &'static str {
    match (l, style % 4) {
        (Legal::Kk, 0) => "株式会社",
        (Legal::Kk, 1) => "(株)",
        (Legal::Kk, 2) => "㈱",
        (Legal::Kk, _) => "（株）",
        (Legal::Yk, 0 | 1) => "有限会社",
        (Legal::Yk, 2) => "(有)",
        (Legal::Yk, _) => "㈲",
        (Legal::Gk, _) => "合同会社",
    }
}

fn en_legal(l: Legal, style: u64) -> &'static str {
    match (l, style % 3) {
        (Legal::Kk, 0) => " Co., Ltd.",
        (Legal::Kk, 1) => " K.K.",
        (Legal::Kk, _) => " Corporation",
        (Legal::Yk, _) => " Y.K.",
        (Legal::Gk, _) => " G.K.",
    }
}

/// Japanese supplier names: `n` entities with 1–4 surface forms each.
pub fn ja_suppliers(seed: u64, n: usize) -> Dataset {
    let mut rng = Rng::new(seed);
    let mut combos: Vec<(usize, usize)> = (0..PLACES.len())
        .flat_map(|p| (0..BUSINESSES.len()).map(move |b| (p, b)))
        .collect();
    // Deterministic shuffle.
    for i in (1..combos.len()).rev() {
        combos.swap(i, rng.below(i as u64 + 1) as usize);
    }
    let mut items = Vec::new();
    let mut used = BTreeSet::new();
    let mut eid = 0u32;
    for &(p, b) in combos.iter().take(n) {
        let (pk, pv, pr, pw) = PLACES[p];
        let (bk, bv, br, bw) = BUSINESSES[b];
        let legal = match rng.below(20) {
            0..=15 => Legal::Kk,
            16..=18 => Legal::Yk,
            _ => Legal::Gk,
        };
        let style = rng.below(4);
        let prefix = rng.below(3) == 0;
        let ja = |core: &str, st: u64, pre: bool| {
            if pre {
                format!("{}{core}", ja_legal(legal, st))
            } else {
                format!("{core}{}", ja_legal(legal, st))
            }
        };
        let kanji = format!("{pk}{bk}");
        let kana = format!("{pr}{br}");
        let mut forms: Vec<(String, &'static str)> = vec![(ja(&kanji, style, prefix), "canonical")];
        let n_var = 1 + rng.below(3);
        let mut tried = BTreeSet::new();
        while (forms.len() as u64) < n_var + 1 && tried.len() < 9 {
            let t = rng.below(9);
            if !tried.insert(t) {
                continue;
            }
            let f = match t {
                0 => (ja(&kanji, style + 1 + rng.below(3), !prefix), "legal_form_notation"),
                1 if pv != pk || bv != bk => (ja(&format!("{pv}{bv}"), style, prefix), "kanji_variant"),
                2 => (format!("{pk}\u{3000}{bk}{}", ja_legal(legal, style)), "fullwidth_space"),
                3 => (kanji.clone(), "legal_form_dropped"),
                4 => (ja(&kana, style, prefix), "katakana_reading"),
                5 => (half_width(&ja(&kana, 1, false)), "halfwidth_katakana"),
                6 if kana != enlarge_small(&kana) || kana != long_vowel_bar(&kana) => (
                    ja(&long_vowel_bar(&enlarge_small(&kana)), style, prefix),
                    "kana_small_or_long_vowel",
                ),
                7 => {
                    let r = format!("{} {}", title(&hepburn_short(pw)), title(&hepburn_short(bw)));
                    (format!("{r}{}", en_legal(legal, rng.below(3))), "romaji")
                }
                8 => {
                    let r = format!("{} {}", title(&macron(pw)), title(&macron(bw))).to_uppercase();
                    (format!("{r}{}", en_legal(legal, rng.below(3))), "romaji_macron_upper")
                }
                _ => continue,
            };
            if !forms.iter().any(|(s, _)| *s == f.0) {
                forms.push(f);
            }
        }
        for (text, variant) in forms {
            if used.insert(text.clone()) {
                items.push(Item {
                    text,
                    entity: eid,
                    variant,
                });
            }
        }
        eid += 1;
    }
    Dataset {
        name: format!("ja_suppliers_seed{seed}_{n}"),
        kind: Kind::Company,
        items,
    }
}

const PART_PREFIXES: &[&str] = &[
    "BRK", "SHF", "HSG", "FLG", "PIN", "GER", "BSH", "CVR", "MTR", "VLV", "SPR", "WSH", "CAM", "RTR", "KZ", "TN", "MS",
    "SUS", "AL", "JIS",
];

fn ocr_swap(s: &str, rng: &mut Rng) -> Option<String> {
    let c: Vec<char> = s.chars().collect();
    let idx: Vec<usize> = (0..c.len()).filter(|&i| "01258".contains(c[i])).collect();
    if idx.is_empty() {
        return None;
    }
    let i = *pick(rng, &idx);
    let mut c2 = c.clone();
    c2[i] = match c[i] {
        '0' => 'O',
        '1' => *pick(rng, &['I', 'l']),
        '2' => 'Z',
        '5' => 'S',
        _ => 'B',
    };
    Some(c2.into_iter().collect())
}

/// Japanese-supplier part numbers: `n` entities, ~20 % with a one-digit-different sibling (a genuinely different
/// part), 0–3 noisy surface forms each.
pub fn ja_parts(seed: u64, n: usize) -> Dataset {
    let mut rng = Rng::new(seed ^ 0x5041_5254);
    let mut canon: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    while canon.len() < n {
        let pn = match rng.below(3) {
            0 => format!("{}-{}", pick(&mut rng, PART_PREFIXES), 1000 + rng.below(90_000)),
            1 => format!(
                "{}-{:04}-{}",
                pick(&mut rng, PART_PREFIXES),
                rng.below(10_000),
                (b'A' + rng.below(6) as u8) as char
            ),
            _ => format!(
                "{}{}-{:03}",
                pick(&mut rng, PART_PREFIXES),
                10 + rng.below(90),
                rng.below(1000)
            ),
        };
        if seen.insert(pn.clone()) {
            canon.push(pn.clone());
        }
        // Sibling: a different part whose number differs in one digit.
        if rng.below(5) == 0 && canon.len() < n {
            let mut c: Vec<char> = pn.chars().collect();
            let digits: Vec<usize> = (0..c.len()).filter(|&i| c[i].is_ascii_digit()).collect();
            let i = *pick(&mut rng, &digits);
            c[i] = char::from_digit((c[i].to_digit(10).unwrap_or(0) + 1 + rng.below(8) as u32) % 10, 10).unwrap_or('0');
            let sib: String = c.into_iter().collect();
            if seen.insert(sib.clone()) {
                canon.push(sib);
            }
        }
    }
    let mut items = Vec::new();
    let mut used = BTreeSet::new();
    for (eid, pn) in canon.iter().enumerate() {
        let mut forms: Vec<(String, &'static str)> = vec![(pn.clone(), "canonical")];
        for _ in 0..rng.below(4) {
            let f = match rng.below(11) {
                0 => (
                    pn.chars()
                        .map(|c| {
                            if c.is_ascii_graphic() {
                                char::from_u32(c as u32 + 0xFEE0).unwrap_or(c)
                            } else {
                                c
                            }
                        })
                        .collect(),
                    "fullwidth",
                ),
                1 => {
                    let k = 1 + rng.below(pn.len() as u64 - 1) as usize;
                    (format!("{}\u{200B}{}", &pn[..k], &pn[k..]), "zero_width")
                }
                2 => (
                    pn.replace('-', pick::<&str>(&mut rng, &["‐", "–", "−", "ー", "ｰ"])),
                    "dash_variant",
                ),
                3 => (pn.replace('-', ""), "separator_removed"),
                4 => (pn.replace('-', " "), "space_separator"),
                5 => (pn.to_lowercase(), "lowercase"),
                6 => (
                    format!("{pn} REV {}", pick(&mut rng, &["A", "B", "C"])),
                    "revision_suffix",
                ),
                7 => (format!("{pn}-R{}", 1 + rng.below(3)), "revision_suffix"),
                8 | 9 => match ocr_swap(pn, &mut rng) {
                    Some(s) => (s, "ocr_confusion"),
                    None => continue,
                },
                _ => {
                    // Rare non-OCR keying error: the hardest case, indistinguishable from a sibling part.
                    if rng.below(4) != 0 {
                        continue;
                    }
                    let mut c: Vec<char> = pn.chars().collect();
                    let digits: Vec<usize> = (0..c.len()).filter(|&i| c[i].is_ascii_digit()).collect();
                    let i = *pick(&mut rng, &digits);
                    c[i] = char::from_digit((c[i].to_digit(10).unwrap_or(0) + 5) % 10, 10).unwrap_or('0');
                    (c.into_iter().collect(), "keying_error")
                }
            };
            if !used.contains(&f.0) && !seen.contains(&f.0) && !forms.iter().any(|(s, _)| *s == f.0) {
                forms.push(f);
            }
        }
        for (text, variant) in forms {
            if used.insert(text.clone()) {
                items.push(Item {
                    text,
                    entity: eid as u32,
                    variant,
                });
            }
        }
    }
    Dataset {
        name: format!("ja_parts_seed{seed}_{n}"),
        kind: Kind::Part,
        items,
    }
}

/// E2E generator ground truth (scripts/e2e/gen_dataset.py): part ids plus the injected duplicate variants.
pub fn from_e2e_truth(name: &str, truth: &serde_json::Value) -> Dataset {
    let mut items = Vec::new();
    let mut eid = BTreeMap::new();
    for p in truth["parts"].as_array().into_iter().flatten() {
        if let Some(pn) = p["part"].as_str() {
            let id = eid.len() as u32;
            eid.insert(pn.to_string(), id);
            items.push(Item {
                text: pn.to_string(),
                entity: id,
                variant: "canonical",
            });
        }
    }
    for pair in truth["duplicate_pairs"].as_array().into_iter().flatten() {
        let (Some(a), Some(b)) = (pair[0].as_str(), pair[1].as_str()) else {
            continue;
        };
        let (known, other) = if eid.contains_key(a) { (a, b) } else { (b, a) };
        if let Some(&id) = eid.get(known) {
            items.push(Item {
                text: other.to_string(),
                entity: id,
                variant: "duplicate_component_id",
            });
        }
    }
    Dataset {
        name: name.to_string(),
        kind: Kind::Part,
        items,
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Score {
    pub true_pairs: usize,
    pub predicted: usize,
    pub tp: usize,
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
}

pub fn score(truth: &BTreeMap<(String, String), &'static str>, predicted: &BTreeSet<(String, String)>) -> Score {
    let tp = predicted.iter().filter(|p| truth.contains_key(*p)).count();
    let precision = if predicted.is_empty() {
        1.0
    } else {
        tp as f64 / predicted.len() as f64
    };
    let recall = if truth.is_empty() {
        1.0
    } else {
        tp as f64 / truth.len() as f64
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    Score {
        true_pairs: truth.len(),
        predicted: predicted.len(),
        tp,
        precision,
        recall,
        f1,
    }
}

pub fn pair(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic_and_consistent() {
        for d in [ja_suppliers(5, 120), ja_parts(5, 300)] {
            let again = if d.kind == Kind::Company {
                ja_suppliers(5, 120)
            } else {
                ja_parts(5, 300)
            };
            assert_eq!(d.keys(), again.keys());
            assert!(d.truth().len() > 50, "{} {}", d.name, d.truth().len());
        }
    }

    #[test]
    fn half_width_round_trips_through_v2_folding() {
        use entity_resolution::v2::normalize::canonical_company;
        assert_eq!(
            canonical_company(&half_width("ダイトウパイプ")),
            canonical_company("ダイトウパイプ")
        );
    }
}
