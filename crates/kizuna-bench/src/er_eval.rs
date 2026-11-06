//! Labelled datasets and scoring for the ER v1 vs v2 comparison (`kizuna-er-compare`).
//!
//! The Japanese supplier/part set is synthetic and deterministic by seed: every surface form carries the id of the
//! entity it was generated from, so pairwise ground truth is exact. Names are invented combinations of common
//! place/business words; no real company list is used.

use entity_resolution::v2::{Kind, PartContext, Rng};
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

/// Places that are deliberately NOT in the ER reading dictionary (`entity_resolution::v2::reading`): the
/// "unseen vocabulary" set measures what dictionary readings cannot do.
const UNSEEN_PLACES: &[(&str, &str, &str, &str)] = &[
    ("甲府", "甲府", "コウフ", "koufu"),
    ("宇都宮", "宇都宮", "ウツノミヤ", "utsunomiya"),
    ("松江", "松江", "マツエ", "matsue"),
    ("鳥取", "鳥取", "トットリ", "tottori"),
    ("高松", "高松", "タカマツ", "takamatsu"),
    ("徳島", "德島", "トクシマ", "tokushima"),
    ("秋田", "秋田", "アキタ", "akita"),
    ("盛岡", "盛岡", "モリオカ", "morioka"),
    ("熊本", "熊本", "クマモト", "kumamoto"),
    ("大分", "大分", "オオイタ", "ooita"),
    ("佐賀", "佐賀", "サガ", "saga"),
    ("福井", "福井", "フクイ", "fukui"),
    ("富山", "富山", "トヤマ", "toyama"),
    ("山形", "山形", "ヤマガタ", "yamagata"),
    ("沼津", "沼津", "ヌマヅ", "numazu"),
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
    /// Evidence context by surface string (part sets only; see `part_contexts`).
    pub context: BTreeMap<String, PartContext>,
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
    ja_suppliers_from(seed, n, PLACES, "ja_suppliers")
}

/// Supplier names whose place words are not in the reading dictionary (at most 225 entities).
pub fn ja_suppliers_unseen(seed: u64, n: usize) -> Dataset {
    ja_suppliers_from(seed, n, UNSEEN_PLACES, "ja_suppliers_unseen")
}

type Word = (&'static str, &'static str, &'static str, &'static str);

fn ja_suppliers_from(seed: u64, n: usize, places: &[Word], label: &str) -> Dataset {
    let mut rng = Rng::new(seed);
    let mut combos: Vec<(usize, usize)> = (0..places.len())
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
        let (pk, pv, pr, pw) = places[p];
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
        name: format!("{label}_seed{seed}_{n}"),
        kind: Kind::Company,
        items,
        context: BTreeMap::new(),
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

/// One noisy surface form of a part number (or `None` when the drawn transform does not apply).
fn noisy_form(pn: &str, rng: &mut Rng) -> Option<(String, &'static str)> {
    Some(match rng.below(11) {
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
            pn.replace('-', pick::<&str>(rng, &["‐", "–", "−", "ー", "ｰ"])),
            "dash_variant",
        ),
        3 => (pn.replace('-', ""), "separator_removed"),
        4 => (pn.replace('-', " "), "space_separator"),
        5 => (pn.to_lowercase(), "lowercase"),
        6 => (format!("{pn} REV {}", pick(rng, &["A", "B", "C"])), "revision_suffix"),
        7 => (format!("{pn}-R{}", 1 + rng.below(3)), "revision_suffix"),
        8 | 9 => match ocr_swap(pn, rng) {
            Some(s) => (s, "ocr_confusion"),
            None => return None,
        },
        _ => {
            // Rare non-OCR keying error: the hardest case, indistinguishable from a sibling part.
            if rng.below(4) != 0 {
                return None;
            }
            let mut c: Vec<char> = pn.chars().collect();
            let digits: Vec<usize> = (0..c.len()).filter(|&i| c[i].is_ascii_digit()).collect();
            let i = *pick(rng, &digits);
            if rng.below(2) == 0 && i + 1 < c.len() && c[i + 1].is_ascii_digit() && c[i] != c[i + 1] {
                c.swap(i, i + 1);
                (c.into_iter().collect(), "keying_transposition")
            } else {
                c[i] = char::from_digit((c[i].to_digit(10).unwrap_or(0) + 5) % 10, 10).unwrap_or('0');
                (c.into_iter().collect(), "keying_error")
            }
        }
    })
}

/// Japanese-supplier part numbers: `n` entities, ~20 % with a one-digit-different sibling (a genuinely different
/// part), 0–3 noisy surface forms each.
pub fn ja_parts(seed: u64, n: usize) -> Dataset {
    let mut rng = Rng::new(seed ^ 0x5041_5254);
    let mut canon: Vec<String> = Vec::new();
    // Entity index of the part a sibling was derived from (context generation only; draws no random numbers).
    let mut sibling_of: Vec<Option<usize>> = Vec::new();
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
            sibling_of.push(None);
        }
        // Sibling: a different part whose number differs in one digit.
        if rng.below(5) == 0 && canon.len() < n {
            let mut c: Vec<char> = pn.chars().collect();
            let digits: Vec<usize> = (0..c.len()).filter(|&i| c[i].is_ascii_digit()).collect();
            let i = *pick(&mut rng, &digits);
            c[i] = char::from_digit((c[i].to_digit(10).unwrap_or(0) + 1 + rng.below(8) as u32) % 10, 10).unwrap_or('0');
            let sib: String = c.into_iter().collect();
            if seen.insert(sib.clone()) {
                // The parent is the entity of `pn` (pushed above, or earlier when `pn` was already seen).
                sibling_of.push(canon.iter().position(|c| *c == pn));
                canon.push(sib);
            }
        }
    }
    let mut items = Vec::new();
    let mut used = BTreeSet::new();
    for (eid, pn) in canon.iter().enumerate() {
        let mut forms: Vec<(String, &'static str)> = vec![(pn.clone(), "canonical")];
        for _ in 0..rng.below(4) {
            let Some(f) = noisy_form(pn, &mut rng) else {
                continue;
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
        context: part_contexts(seed, &canon, &sibling_of, &items),
        items,
    }
}

/// Part family by prefix: English noun, Japanese noun, size letter, typical materials.
fn family(prefix: &str) -> (&'static str, &'static str, &'static str, &'static [&'static str]) {
    match prefix {
        "BRK" => ("BRACKET", "ブラケット", "L", &["SS400", "SPCC", "SUS304"]),
        "SHF" => ("SHAFT", "シャフト", "φ", &["S45C", "SCM440", "SUS303"]),
        "HSG" => ("HOUSING", "ハウジング", "W", &["ADC12", "FC250"]),
        "FLG" => ("FLANGE", "フランジ", "φ", &["SS400", "SUS304"]),
        "PIN" => ("PIN", "ピン", "φ", &["S45C", "SUS304", "SK4"]),
        "GER" => ("GEAR", "歯車", "M", &["S45C", "SCM415"]),
        "BSH" => ("BUSHING", "ブッシュ", "φ", &["C3604", "POM"]),
        "CVR" => ("COVER", "カバー", "W", &["SPCC", "A5052", "ABS"]),
        "MTR" => ("MOTOR MOUNT", "モーター台", "W", &["SS400", "A5052"]),
        "VLV" => ("VALVE BODY", "バルブ本体", "φ", &["SCS13", "C3771"]),
        "SPR" => ("SPRING", "ばね", "φ", &["SWP-B", "SUS304-WPB"]),
        "WSH" => ("WASHER", "座金", "φ", &["SPCC", "SUS304"]),
        "CAM" => ("CAM", "カム", "R", &["SCM440", "S50C"]),
        "RTR" => ("ROTOR", "ロータ", "φ", &["S45C", "SUS420J2"]),
        "SUS" => ("PLATE", "プレート", "T", &["SUS304", "SUS316"]),
        "AL" => ("BLOCK", "ブロック", "W", &["A5052", "A6061", "A7075"]),
        _ => ("PART", "部品", "L", &["SS400", "S45C", "SUS304", "A5052"]),
    }
}

/// Invented supplier names (place + business words from the supplier set), so supplier context can be written in
/// the forms the supplier normaliser folds.
fn supplier_pool(rng: &mut Rng, k: usize) -> Vec<String> {
    let mut v = BTreeSet::new();
    while v.len() < k {
        v.insert(format!("{}{}", pick(rng, PLACES).0, pick(rng, BUSINESSES).0));
    }
    v.into_iter().collect()
}

fn alpha_prefix(pn: &str) -> String {
    pn.chars().take_while(|c| c.is_ascii_alphabetic()).collect()
}

/// Evidence context for every surface form of a part set (`er_eval::ja_parts*`).
///
/// Assumptions (SIMULATED; invented, not measured on a real part master):
/// - each entity has a master record: description `<noun> <size>` (English or Japanese, 50/50), one supplier
///   from a pool of 40, its own drawing number (`D-` + 6 digits) and a family material;
/// - a sibling (a different part one digit away) keeps the parent's supplier with p = 0.7, its material with
///   p = 0.7, its drawing with p = 0.15 (tabulated drawing) and its size with p = 0.15;
/// - the canonical form carries the full master record; every other surface form is a document occurrence that
///   shows each field independently (description 0.5, supplier 0.85, drawing 0.6, material 0.5), writes the
///   description in either language (50/50), and names a different (second-source) supplier with p = 0.03.
///
/// Uses its own random stream, so the keys and truth of a dataset do not depend on it.
pub fn part_contexts(
    seed: u64,
    canon: &[String],
    sibling_of: &[Option<usize>],
    items: &[Item],
) -> BTreeMap<String, PartContext> {
    struct Master {
        noun: (&'static str, &'static str),
        size: String,
        supplier: String,
        drawing: String,
        material: &'static str,
    }
    let mut rng = Rng::new(seed ^ 0x4354_5854_2026_1105);
    let suppliers = supplier_pool(&mut rng, 40);
    let mut masters: Vec<Master> = Vec::with_capacity(canon.len());
    for (e, pn) in canon.iter().enumerate() {
        let (en, ja, letter, mats) = family(&alpha_prefix(pn));
        let fresh_size = |rng: &mut Rng| format!("{letter}{}", 3 + rng.below(198));
        let mut m = Master {
            noun: (en, ja),
            size: fresh_size(&mut rng),
            supplier: pick(&mut rng, &suppliers).clone(),
            drawing: format!("D-{:06}", rng.below(1_000_000)),
            material: mats[rng.below(mats.len() as u64) as usize],
        };
        if let Some(p) = sibling_of.get(e).copied().flatten().and_then(|p| masters.get(p)) {
            if rng.unit() < 0.7 {
                m.supplier = p.supplier.clone();
            }
            if rng.unit() < 0.7 {
                m.material = p.material;
            }
            if rng.unit() < 0.15 {
                m.drawing = p.drawing.clone();
            }
            if rng.unit() < 0.15 {
                m.size = p.size.clone();
            }
        }
        masters.push(m);
    }
    let mut out = BTreeMap::new();
    for it in items {
        let m = &masters[it.entity as usize];
        let desc = |rng: &mut Rng| {
            let noun = if rng.below(2) == 0 { m.noun.0 } else { m.noun.1 };
            format!("{noun} {}", m.size)
        };
        let c = if it.variant == "canonical" {
            PartContext {
                descriptions: vec![desc(&mut rng)],
                suppliers: vec![m.supplier.clone()],
                drawings: vec![m.drawing.clone()],
                materials: vec![m.material.to_string()],
            }
        } else {
            let mut c = PartContext::default();
            if rng.unit() < 0.5 {
                c.descriptions.push(desc(&mut rng));
            }
            if rng.unit() < 0.85 {
                c.suppliers.push(if rng.unit() < 0.03 {
                    pick(&mut rng, &suppliers).clone()
                } else {
                    m.supplier.clone()
                });
            }
            if rng.unit() < 0.6 {
                c.drawings.push(m.drawing.clone());
            }
            if rng.unit() < 0.5 {
                c.materials.push(m.material.to_string());
            }
            c
        };
        out.insert(it.text.clone(), c);
    }
    out
}

/// A sparser part master than `ja_parts` (SIMULATED; assumptions, not measurements):
/// - 12 prefixes (product families), numbers drawn at random from a 6-digit space per prefix (e.g. legacy masters
///   or drawing-derived numbers; sequentially allocated masters are denser and closer to `ja_parts`);
/// - two formats: `BRK-123456` and `BRK123-456`;
/// - 5 % of parts have a hand/variant sibling whose number differs in the last digit (a different part);
/// - most keys are clean: 0–2 noisy surface forms per part (P = 0.55 / 0.30 / 0.15), drawn from the same
///   transforms as `ja_parts` (including the rare keying typos).
pub fn ja_parts_sparse(seed: u64, n: usize) -> Dataset {
    let mut rng = Rng::new(seed ^ 0x5350_4152_5345);
    let prefixes: Vec<&str> = PART_PREFIXES.iter().copied().take(12).collect();
    let mut canon: Vec<String> = Vec::new();
    let mut sibling_of: Vec<Option<usize>> = Vec::new();
    let mut seen = BTreeSet::new();
    let fmt = |p: &str, num: u64, style: u64| {
        if style == 0 {
            format!("{p}-{num:06}")
        } else {
            format!("{p}{:03}-{:03}", num / 1000, num % 1000)
        }
    };
    while canon.len() < n {
        let p = *pick(&mut rng, &prefixes);
        let style = u64::from(p.len() > 2 && rng.below(3) == 0);
        let num = 100_000 + rng.below(900_000);
        let pn = fmt(p, num, style);
        if !seen.insert(pn.clone()) {
            continue;
        }
        canon.push(pn.clone());
        sibling_of.push(None);
        if rng.unit() < 0.05 && canon.len() < n {
            let sib = fmt(p, num - num % 10 + (num % 10 + 1 + rng.below(8)) % 10, style);
            if seen.insert(sib.clone()) {
                sibling_of.push(Some(canon.len() - 1));
                canon.push(sib);
            }
        }
    }
    let mut items = Vec::new();
    let mut used = BTreeSet::new();
    for (eid, pn) in canon.iter().enumerate() {
        let mut forms: Vec<(String, &'static str)> = vec![(pn.clone(), "canonical")];
        let u = rng.unit();
        let k = if u < 0.55 {
            0
        } else if u < 0.85 {
            1
        } else {
            2
        };
        for _ in 0..k {
            let Some(f) = noisy_form(pn, &mut rng) else {
                continue;
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
        name: format!("ja_parts_sparse_seed{seed}_{n}"),
        kind: Kind::Part,
        context: part_contexts(seed, &canon, &sibling_of, &items),
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
        context: BTreeMap::new(),
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
    fn dense_set_is_unchanged_by_context_generation() {
        // Key and truth counts of the 2026-11-05 report (reports/er/er_v2_compare.json at 1668d37).
        let d = ja_parts(20261105, 1000);
        assert_eq!((d.keys().len(), d.truth().len()), (2307, 2055));
        assert_eq!(d.context.len(), d.keys().len());
    }

    #[test]
    fn sparse_set_is_deterministic_sparser_and_has_context() {
        let (a, b) = (ja_parts_sparse(9, 2000), ja_parts_sparse(9, 2000));
        assert_eq!(a.keys(), b.keys());
        assert_eq!(a.context, b.context);
        assert_eq!(a.context.len(), a.keys().len());
        let dense = ja_parts(9, 2000);
        let per_key = |d: &Dataset| d.truth().len() as f64 / d.keys().len() as f64;
        assert!(per_key(&a) < per_key(&dense), "{} vs {}", per_key(&a), per_key(&dense));
        assert!(a.items.iter().any(|i| i.variant.starts_with("keying")));
        // Canonical forms carry the full master record.
        let c = a.items.iter().find(|i| i.variant == "canonical").unwrap();
        let m = &a.context[&c.text];
        assert!(
            !m.descriptions.is_empty() && !m.suppliers.is_empty() && !m.drawings.is_empty() && !m.materials.is_empty()
        );
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
