//! Field normalisers for ER v2. Every function here is idempotent (property-tested) and never consults data
//! outside its argument, so the same input always yields the same key.

/// Invisible characters that copy-paste and OCR insert into identifiers.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{2060}' | '\u{FEFF}' | '\u{00AD}' | '\u{180E}'
    )
}

/// Full-width ASCII → ASCII, ideographic/no-break spaces → space.
fn fold_width_ascii(c: char) -> char {
    match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{3000}' | '\u{00A0}' | '\u{2002}'..='\u{200A}' => ' ',
        _ => c,
    }
}

/// Hyphen-like characters seen in Japanese part numbers (including the katakana long-vowel mark, which IMEs
/// produce when a hyphen is typed in kana mode).
fn is_dash(c: char) -> bool {
    matches!(
        c,
        '\u{2010}'..='\u{2015}' | '\u{2212}' | '\u{2043}' | '\u{FE58}' | '\u{FE63}' | '\u{30FC}' | '\u{FF70}'
    )
}

/// Canonical part number: invisible characters removed, width folded, dash variants → '-', whitespace collapsed,
/// upper-cased. Separators are kept (BRK-100 and BRK100 may be different parts).
pub fn part_key(s: &str) -> String {
    let mapped: String = s
        .chars()
        .filter(|c| !is_invisible(*c))
        .map(fold_width_ascii)
        .map(|c| if is_dash(c) { '-' } else { c })
        .collect();
    let up = mapped.split_whitespace().collect::<Vec<_>>().join(" ").to_uppercase();
    // Upper-casing can (rarely) produce characters the first pass would map; a second pass makes this a fixpoint.
    if up
        .chars()
        .any(|c| is_invisible(c) || is_dash(c) || fold_width_ascii(c) != c)
    {
        return part_key(&up);
    }
    up
}

/// Part number without separators.
pub fn part_loose(s: &str) -> String {
    part_key(s).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// OCR-confusable letters folded to the digit they are usually misread as (only meaningful next to digits).
pub fn ocr_fold(loose: &str) -> String {
    loose
        .chars()
        .map(|c| match c {
            'O' | 'Q' | 'D' => '0',
            'I' | 'L' => '1',
            'Z' => '2',
            'S' => '5',
            'G' => '6',
            'B' => '8',
            _ => c,
        })
        .collect()
}

/// Splits a trailing revision marker (" REV C", "-REV.B", "_R3") from a part key.
pub fn split_revision(key: &str) -> (String, Option<String>) {
    for marker in [" REV ", "-REV", "_REV", " REV.", " REV", " R", "-R", "_R"] {
        if let Some(pos) = key.rfind(marker) {
            let tail = key[pos + marker.len()..].trim_start_matches(['.', ' ']);
            if pos > 0 && !tail.is_empty() && tail.len() <= 3 && tail.chars().all(|c| c.is_ascii_alphanumeric()) {
                return (key[..pos].to_string(), Some(tail.to_string()));
            }
        }
    }
    (key.to_string(), None)
}

// ---------------------------------------------------------------------------------------------------------------
// Japanese company names
// ---------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegalForm {
    /// 株式会社 / (株) / ㈱ / K.K. / Co., Ltd. / Inc. / Corp.
    Kabushiki,
    /// 有限会社 / (有) / ㈲ / Y.K.
    Yugen,
    /// 合同会社 / (同) / G.K. / LLC
    Godo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyKey {
    /// Folded name without legal form, spaces or punctuation.
    pub core: String,
    /// Canonical romaji when the name is kana or Latin (None for names containing kanji: no reading dictionary).
    pub romaji: Option<String>,
    pub legal: Option<LegalForm>,
}

const HALF_KATAKANA: &str =
    "ヲァィゥェォャュョッーアイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワン";
const VOICEABLE: &str = "カキクケコサシスセソタチツテトハヒフヘホ";
const SEMI_VOICEABLE: &str = "ハヒフヘホ";

/// Itaiji (variant kanji common in company and family names) → standard form.
const KANJI_VARIANTS: &[(char, char)] = &[
    ('髙', '高'),
    ('﨑', '崎'),
    ('嵜', '崎'),
    ('碕', '崎'),
    ('齋', '斎'),
    ('齊', '斎'),
    ('斉', '斎'),
    ('邊', '辺'),
    ('邉', '辺'),
    ('澤', '沢'),
    ('濱', '浜'),
    ('國', '国'),
    ('廣', '広'),
    ('櫻', '桜'),
    ('會', '会'),
    ('關', '関'),
    ('鐵', '鉄'),
    ('德', '徳'),
    ('槇', '槙'),
    ('眞', '真'),
    ('藏', '蔵'),
    ('惠', '恵'),
    ('冨', '富'),
    ('舘', '館'),
    ('嶋', '島'),
    ('嶌', '島'),
    ('峯', '峰'),
    ('榮', '栄'),
    ('曾', '曽'),
    ('瀨', '瀬'),
    ('增', '増'),
    ('實', '実'),
    ('豐', '豊'),
    ('寶', '宝'),
    ('學', '学'),
    ('黑', '黒'),
    ('氣', '気'),
    ('檜', '桧'),
    ('萬', '万'),
    ('與', '与'),
    ('條', '条'),
    ('發', '発'),
    ('鑄', '鋳'),
    ('靜', '静'),
    ('淺', '浅'),
    ('驛', '駅'),
    ('縣', '県'),
    ('圓', '円'),
    ('號', '号'),
    ('辨', '弁'),
    ('屬', '属'),
];

fn kanji_standard(c: char) -> char {
    KANJI_VARIANTS.iter().find(|(v, _)| *v == c).map_or(c, |(_, s)| *s)
}

fn is_katakana(c: char) -> bool {
    ('\u{30A1}'..='\u{30FA}').contains(&c) || c == 'ー'
}

fn is_kanji(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
        || ('\u{3400}'..='\u{4DBF}').contains(&c)
        || ('\u{F900}'..='\u{FAFF}').contains(&c)
}

/// Width folding for names: full-width ASCII → ASCII, half-width katakana → full-width (dakuten merged),
/// hiragana → katakana, combining/stand-alone voicing marks merged, invisible characters dropped.
fn fold_script(s: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars().filter(|c| !is_invisible(*c)) {
        let c = fold_width_ascii(c);
        let c = match c {
            'ā' | 'â' | 'Ā' | 'Â' => 'a',
            'ī' | 'î' | 'Ī' | 'Î' => 'i',
            'ū' | 'û' | 'Ū' | 'Û' => 'u',
            'ē' | 'ê' | 'Ē' | 'Ê' => 'e',
            'ō' | 'ô' | 'Ō' | 'Ô' => 'o',
            '\u{FF66}'..='\u{FF9D}' => HALF_KATAKANA.chars().nth((c as u32 - 0xFF66) as usize).unwrap_or(c),
            '\u{3041}'..='\u{3096}' => char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        };
        let voicing = match c {
            '\u{FF9E}' | '\u{3099}' | '\u{309B}' => Some(1),
            '\u{FF9F}' | '\u{309A}' | '\u{309C}' => Some(2),
            _ => None,
        };
        let Some(v) = voicing else {
            out.push(kanji_standard(c));
            continue;
        };
        if let Some(p) = out.last_mut() {
            *p = match (v, *p) {
                (1, 'ウ') => 'ヴ',
                (1, q) if VOICEABLE.contains(q) => char::from_u32(q as u32 + 1).unwrap_or(q),
                (2, q) if SEMI_VOICEABLE.contains(q) => char::from_u32(q as u32 + 2).unwrap_or(q),
                (_, q) => q,
            };
        }
    }
    out.into_iter().collect()
}

const JA_FORMS: &[(&str, LegalForm)] = &[
    ("株式会社", LegalForm::Kabushiki),
    ("(株)", LegalForm::Kabushiki),
    ("㈱", LegalForm::Kabushiki),
    ("カブシキガイシャ", LegalForm::Kabushiki),
    ("カブシキカイシャ", LegalForm::Kabushiki),
    ("カブシキガイシヤ", LegalForm::Kabushiki),
    ("カブシキカイシヤ", LegalForm::Kabushiki),
    ("有限会社", LegalForm::Yugen),
    ("(有)", LegalForm::Yugen),
    ("㈲", LegalForm::Yugen),
    ("合同会社", LegalForm::Godo),
    ("(同)", LegalForm::Godo),
];

/// Latin legal-form tokens, matched on whole tokens at the end (or start) of the name after punctuation removal.
const LATIN_FORMS: &[(&[&str], LegalForm)] = &[
    (&["KABUSHIKI", "KAISHA"], LegalForm::Kabushiki),
    (&["KABUSHIKIKAISHA"], LegalForm::Kabushiki),
    (&["CO", "LTD"], LegalForm::Kabushiki),
    (&["COMPANY", "LIMITED"], LegalForm::Kabushiki),
    (&["CORPORATION"], LegalForm::Kabushiki),
    (&["CORP"], LegalForm::Kabushiki),
    (&["INC"], LegalForm::Kabushiki),
    (&["KK"], LegalForm::Kabushiki),
    (&["LTD"], LegalForm::Kabushiki),
    (&["LIMITED"], LegalForm::Kabushiki),
    (&["YUGEN", "KAISHA"], LegalForm::Yugen),
    (&["YK"], LegalForm::Yugen),
    (&["GODO", "KAISHA"], LegalForm::Godo),
    (&["GK"], LegalForm::Godo),
    (&["LLC"], LegalForm::Godo),
];

/// Removes one legal form (Japanese anywhere, Latin as leading/trailing tokens). Returns None when nothing found.
fn strip_legal_once(s: &str) -> Option<(String, LegalForm)> {
    for (form, lf) in JA_FORMS {
        if let Some(pos) = s.find(form) {
            return Some((format!("{}{}", &s[..pos], &s[pos + form.len()..]), *lf));
        }
    }
    // Latin: tokens separated by anything that is not alphanumeric ("Co., Ltd." → CO LTD; "K.K." → K K → KK).
    let upper = s.to_uppercase();
    let tokens: Vec<String> = upper
        .split(|c: char| !c.is_alphanumeric() && c != '.')
        .filter(|t| !t.is_empty())
        .map(|t| t.replace('.', ""))
        .filter(|t| !t.is_empty())
        .collect();
    // Dotted abbreviations such as "K.K." became "KK" above; "Co.,Ltd." became "CO" + "LTD".
    for (form, lf) in LATIN_FORMS {
        let n = form.len();
        if tokens.len() > n
            && tokens[tokens.len() - n..]
                .iter()
                .map(String::as_str)
                .eq(form.iter().copied())
        {
            return Some((tokens[..tokens.len() - n].join(" "), *lf));
        }
        if tokens.len() > n && tokens[..n].iter().map(String::as_str).eq(form.iter().copied()) {
            return Some((tokens[n..].join(" "), *lf));
        }
    }
    None
}

/// Kana folding for comparison: small kana → large, long-vowel mark dropped, ウ after an o-row kana dropped
/// (セイコウ / セイコー / セイコ all fold to セイコ).
fn fold_kana(s: &str) -> String {
    const SMALL: &str = "ァィゥェォッャュョヮヵヶ";
    const LARGE: &str = "アイウエオツヤユヨワカケ";
    const O_ROW: &str = "オコソトノホモヨロゴゾドボポヲ";
    let mut out = String::with_capacity(s.len());
    let mut last: Option<char> = None;
    for c in s.chars() {
        let c = SMALL
            .chars()
            .position(|x| x == c)
            .and_then(|i| LARGE.chars().nth(i))
            .unwrap_or(c);
        if c == 'ー' || (c == 'ウ' && last.is_some_and(|p| O_ROW.contains(p))) {
            continue;
        }
        out.push(c);
        last = Some(c);
    }
    out
}

const ROMAJI: &[(char, &str)] = &[
    ('ア', "a"),
    ('イ', "i"),
    ('ウ', "u"),
    ('エ', "e"),
    ('オ', "o"),
    ('カ', "ka"),
    ('キ', "ki"),
    ('ク', "ku"),
    ('ケ', "ke"),
    ('コ', "ko"),
    ('ガ', "ga"),
    ('ギ', "gi"),
    ('グ', "gu"),
    ('ゲ', "ge"),
    ('ゴ', "go"),
    ('サ', "sa"),
    ('シ', "shi"),
    ('ス', "su"),
    ('セ', "se"),
    ('ソ', "so"),
    ('ザ', "za"),
    ('ジ', "ji"),
    ('ズ', "zu"),
    ('ゼ', "ze"),
    ('ゾ', "zo"),
    ('タ', "ta"),
    ('チ', "chi"),
    ('ツ', "tsu"),
    ('テ', "te"),
    ('ト', "to"),
    ('ダ', "da"),
    ('ヂ', "ji"),
    ('ヅ', "zu"),
    ('デ', "de"),
    ('ド', "do"),
    ('ナ', "na"),
    ('ニ', "ni"),
    ('ヌ', "nu"),
    ('ネ', "ne"),
    ('ノ', "no"),
    ('ハ', "ha"),
    ('ヒ', "hi"),
    ('フ', "fu"),
    ('ヘ', "he"),
    ('ホ', "ho"),
    ('バ', "ba"),
    ('ビ', "bi"),
    ('ブ', "bu"),
    ('ベ', "be"),
    ('ボ', "bo"),
    ('パ', "pa"),
    ('ピ', "pi"),
    ('プ', "pu"),
    ('ペ', "pe"),
    ('ポ', "po"),
    ('マ', "ma"),
    ('ミ', "mi"),
    ('ム', "mu"),
    ('メ', "me"),
    ('モ', "mo"),
    ('ヤ', "ya"),
    ('ユ', "yu"),
    ('ヨ', "yo"),
    ('ラ', "ra"),
    ('リ', "ri"),
    ('ル', "ru"),
    ('レ', "re"),
    ('ロ', "ro"),
    ('ワ', "wa"),
    ('ヰ', "i"),
    ('ヱ', "e"),
    ('ヲ', "o"),
    ('ン', "n"),
    ('ヴ', "vu"),
    ('ァ', "a"),
    ('ィ', "i"),
    ('ゥ', "u"),
    ('ェ', "e"),
    ('ォ', "o"),
    ('ャ', "ya"),
    ('ュ', "yu"),
    ('ョ', "yo"),
    ('ヮ', "wa"),
    ('ヵ', "ka"),
    ('ヶ', "ke"),
];

/// Hepburn romaji of a katakana string (yōon, sokuon and long-vowel marks handled). None if any other character.
fn kana_to_romaji(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut geminate = false;
    for c in s.chars() {
        match c {
            'ッ' => geminate = true,
            'ー' => {
                if let Some(v) = out.chars().last().filter(|v| "aeiou".contains(*v)) {
                    out.push(v);
                }
            }
            'ャ' | 'ュ' | 'ョ' if out.ends_with('i') => {
                out.pop();
                let v = &ROMAJI.iter().find(|(k, _)| *k == c)?.1[1..];
                if !(out.ends_with("sh") || out.ends_with("ch") || out.ends_with('j')) {
                    out.push('y');
                }
                out.push_str(v);
            }
            'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' if out.ends_with(|v: char| "aeiou".contains(v)) => {
                out.pop();
                out.push_str(ROMAJI.iter().find(|(k, _)| *k == c)?.1);
            }
            _ => {
                let r = ROMAJI.iter().find(|(k, _)| *k == c)?.1;
                if std::mem::take(&mut geminate) {
                    out.push(if r.starts_with("ch") { 't' } else { r.chars().next()? });
                }
                out.push_str(r);
            }
        }
    }
    Some(out)
}

/// Canonical romaji: Hepburn/Kunrei/wāpuro spellings and long vowels folded so "Kantou", "Kantō", "Kanto" agree.
pub fn fold_romaji(s: &str) -> String {
    let mut r: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'ā' | 'â' => 'a',
            'ī' | 'î' => 'i',
            'ū' | 'û' => 'u',
            'ē' | 'ê' => 'e',
            'ō' | 'ô' => 'o',
            _ => c,
        })
        .filter(char::is_ascii_alphanumeric)
        .collect();
    // Order matters: spelling systems first, then long vowels. Loop to a fixpoint so the result is idempotent.
    loop {
        let before = r.clone();
        for (from, to) in [
            ("sh", "s"),
            ("ch", "t"),
            ("ts", "t"),
            ("j", "zy"),
            ("fu", "hu"),
            ("mb", "nb"),
            ("mp", "np"),
            ("mm", "nm"),
            ("oh", "o"),
            ("ou", "o"),
            ("oo", "o"),
            ("uu", "u"),
            ("aa", "a"),
            ("ii", "i"),
            ("ee", "e"),
            ("zyi", "zi"),
            // Enlarged yōon (シヨウ for ショウ, as in old registered names) reads as -iyo-.
            ("iya", "ya"),
            ("iyu", "yu"),
            ("iyo", "yo"),
        ] {
            r = r.replace(from, to);
        }
        if r == before {
            return r;
        }
    }
}

/// Folds a company name into comparison keys.
pub fn company_key(s: &str) -> CompanyKey {
    let mut name = fold_script(s);
    let mut legal = None;
    while let Some((rest, lf)) = strip_legal_once(&name) {
        legal.get_or_insert(lf);
        name = rest;
    }
    let letters: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || is_katakana(*c))
        .collect();
    // Fixpoint: punctuation removal or kana folding can expose another legal form ("株式・会社").
    let mut core = fold_kana(&letters);
    while let Some((rest, lf)) = JA_FORMS.iter().find_map(|(f, lf)| {
        core.find(f)
            .map(|p| (format!("{}{}", &core[..p], &core[p + f.len()..]), *lf))
    }) {
        legal.get_or_insert(lf);
        core = fold_kana(&rest);
    }
    let romaji = if letters.is_empty() {
        None
    } else if letters.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(fold_romaji(&name))
    } else if letters.chars().all(is_katakana) {
        kana_to_romaji(&letters).map(|r| fold_romaji(&r))
    } else {
        None
    };
    CompanyKey { core, romaji, legal }
}

/// Canonical company name (core only); `canonical_company(canonical_company(x)) == canonical_company(x)`.
pub fn canonical_company(s: &str) -> String {
    company_key(s).core
}

/// True if the name contains kanji (no reading available → romaji comparison impossible).
pub fn has_kanji(s: &str) -> bool {
    s.chars().any(is_kanji)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_keys() {
        assert_eq!(part_key("ＢＲＫ－１００"), "BRK-100");
        assert_eq!(part_key("BRK\u{200B}-100"), "BRK-100");
        assert_eq!(part_key("BRK–100"), "BRK-100");
        assert_eq!(part_key("BRKー100"), "BRK-100");
        assert_eq!(part_key("brkｰ100"), "BRK-100");
        assert_eq!(part_key("  brk  100 "), "BRK 100");
        assert_eq!(part_loose("BRK-100 REV C"), "BRK100REVC");
        assert_eq!(split_revision("BRK-100 REV C"), ("BRK-100".into(), Some("C".into())));
        assert_eq!(split_revision("BRK-100"), ("BRK-100".into(), None));
        assert_eq!(ocr_fold("BRK10O0"), "8RK1000");
    }

    #[test]
    fn company_legal_forms() {
        for s in [
            "株式会社東海精機",
            "東海精機株式会社",
            "(株)東海精機",
            "（株）東海精機",
            "㈱東海精機",
            "東海精機㈱",
            "東海 精機 (株)",
        ] {
            let k = company_key(s);
            assert_eq!(k.core, "東海精機", "{s}");
            assert_eq!(k.legal, Some(LegalForm::Kabushiki), "{s}");
        }
        assert_eq!(company_key("有限会社東海精機").legal, Some(LegalForm::Yugen));
        assert_eq!(company_key("東海精機").legal, None);
        let k = company_key("Tokai Seiki Co., Ltd.");
        assert_eq!((k.core.as_str(), k.legal), ("TOKAISEIKI", Some(LegalForm::Kabushiki)));
        assert_eq!(company_key("TOKAI SEIKI K.K.").core, "TOKAISEIKI");
        assert_eq!(company_key("Tokai Seiki Inc").legal, Some(LegalForm::Kabushiki));
    }

    #[test]
    fn company_kana_and_kanji_folding() {
        assert_eq!(canonical_company("髙橋製作所"), canonical_company("高橋製作所"));
        assert_eq!(canonical_company("山﨑金属"), canonical_company("山崎金属"));
        assert_eq!(canonical_company("ｾｲｺｰ"), canonical_company("セイコー"));
        assert_eq!(canonical_company("ﾀﾞｲﾄｳ"), canonical_company("ダイトウ"));
        assert_eq!(canonical_company("せいこう"), canonical_company("セイコー"));
        assert_eq!(canonical_company("ニッポン"), canonical_company("ニツポン"));
        assert_eq!(canonical_company("キャノン"), canonical_company("キヤノン"));
    }

    #[test]
    fn romaji_fallback() {
        let r = |s: &str| company_key(s).romaji;
        assert_eq!(r("カントウセイミツ"), r("Kanto Seimitsu"));
        assert_eq!(r("カントーセイミツ"), r("KANTOU SEIMITSU"));
        assert_eq!(r("ニッポンハツジョウ"), r("Nippon Hatsujo"));
        assert_eq!(r("シンバシ"), r("Simbasi"));
        assert_eq!(r("チュウゾウ"), r("Chuzo"));
        assert!(r("東海精機").is_none());
    }
}
