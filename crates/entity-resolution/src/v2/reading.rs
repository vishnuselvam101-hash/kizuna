//! Dictionary readings for kanji company names (ER v3 blocking and comparison).
//!
//! A kanji name has no romaji, so before this module a kanji form could never meet its katakana or Latin form
//! (株式会社東海精機 / トウカイセイキ / Tokai Seiki Co., Ltd.): no shared blocking key, no comparable field. Here a
//! small built-in word list of common company-name words (business types, prefectures and cities, common family
//! names) gives katakana readings; the kanji core is segmented greedily (longest word first, katakana passes
//! through) and, only when EVERY character is covered, each reading becomes a folded romaji key.
//!
//! A dictionary reading is a guess, not evidence of identity: 工業 and 興業 both read コウギョウ, and 日本 reads
//! ニッポン or ニホン. Readings therefore only (a) add blocking keys and (b) feed a separate comparison field used
//! when exactly one side's reading is derived here (kanji vs. kana/Latin). Two kanji names are never compared by
//! derived reading. Nothing is merged; a human still confirms every pair.
//!
//! Names with any kanji outside the list get no reading (same behaviour as before).

/// (kanji word, katakana readings; the first is the most common). Kanji are in standard form (itaiji are folded
/// before lookup).
const WORDS: &[(&str, &[&str])] = &[
    // Business-type words.
    ("精機", &["セイキ"]),
    ("精密", &["セイミツ"]),
    ("精工", &["セイコウ"]),
    ("金属", &["キンゾク"]),
    ("工業", &["コウギョウ"]),
    ("興業", &["コウギョウ"]),
    ("鋼業", &["コウギョウ"]),
    ("工業所", &["コウギョウショ"]),
    ("製作所", &["セイサクショ"]),
    ("工作所", &["コウサクショ"]),
    ("鉄工所", &["テッコウショ"]),
    ("製鋼所", &["セイコウショ"]),
    ("研究所", &["ケンキュウショ"]),
    ("電機", &["デンキ"]),
    ("電気", &["デンキ"]),
    ("電子", &["デンシ"]),
    ("電工", &["デンコウ"]),
    ("電装", &["デンソウ"]),
    ("化成", &["カセイ"]),
    ("化学", &["カガク"]),
    ("化工", &["カコウ"]),
    ("鋳造", &["チュウゾウ"]),
    ("鍛造", &["タンゾウ"]),
    ("発条", &["ハツジョウ"]),
    ("産業", &["サンギョウ"]),
    ("工機", &["コウキ"]),
    ("鉄工", &["テッコウ"]),
    ("鉄鋼", &["テッコウ"]),
    ("製鋼", &["セイコウ"]),
    ("機械", &["キカイ"]),
    ("機工", &["キコウ"]),
    ("技研", &["ギケン"]),
    ("技術", &["ギジュツ"]),
    ("商事", &["ショウジ"]),
    ("商会", &["ショウカイ"]),
    ("商店", &["ショウテン"]),
    ("物産", &["ブッサン"]),
    ("興産", &["コウサン"]),
    ("通商", &["ツウショウ"]),
    ("貿易", &["ボウエキ"]),
    ("製作", &["セイサク"]),
    ("製造", &["セイゾウ"]),
    ("製薬", &["セイヤク"]),
    ("樹脂", &["ジュシ"]),
    ("塗装", &["トソウ"]),
    ("研磨", &["ケンマ"]),
    ("熱処理", &["ネツショリ"]),
    ("部品", &["ブヒン"]),
    ("工具", &["コウグ"]),
    ("計器", &["ケイキ"]),
    ("光学", &["コウガク"]),
    ("硝子", &["ガラス"]),
    ("印刷", &["インサツ"]),
    ("包装", &["ホウソウ"]),
    ("開発", &["カイハツ"]),
    ("建設", &["ケンセツ"]),
    ("運輸", &["ウンユ"]),
    ("重工", &["ジュウコウ"]),
    ("重工業", &["ジュウコウギョウ"]),
    ("自動車", &["ジドウシャ"]),
    ("工芸", &["コウゲイ"]),
    ("鍍金", &["メッキ"]),
    // Regions, prefectures and cities common in company names.
    ("日本", &["ニッポン", "ニホン"]),
    ("東海", &["トウカイ"]),
    ("関東", &["カントウ"]),
    ("関西", &["カンサイ"]),
    ("東北", &["トウホク"]),
    ("北陸", &["ホクリク"]),
    ("中部", &["チュウブ"]),
    ("九州", &["キュウシュウ"]),
    ("四国", &["シコク"]),
    ("北海道", &["ホッカイドウ"]),
    ("東京", &["トウキョウ"]),
    ("京都", &["キョウト"]),
    ("大阪", &["オオサカ"]),
    ("名古屋", &["ナゴヤ"]),
    ("横浜", &["ヨコハマ"]),
    ("神戸", &["コウベ"]),
    ("川崎", &["カワサキ"]),
    ("千葉", &["チバ"]),
    ("埼玉", &["サイタマ"]),
    ("静岡", &["シズオカ"]),
    ("浜松", &["ハママツ"]),
    ("長野", &["ナガノ"]),
    ("岡山", &["オカヤマ"]),
    ("広島", &["ヒロシマ"]),
    ("福岡", &["フクオカ"]),
    ("仙台", &["センダイ"]),
    ("新潟", &["ニイガタ"]),
    ("金沢", &["カナザワ"]),
    ("岐阜", &["ギフ"]),
    ("三重", &["ミエ"]),
    ("奈良", &["ナラ"]),
    ("姫路", &["ヒメジ"]),
    ("豊田", &["トヨタ"]),
    ("刈谷", &["カリヤ"]),
    ("京浜", &["ケイヒン"]),
    ("阪神", &["ハンシン"]),
    ("中京", &["チュウキョウ"]),
    ("新橋", &["シンバシ"]),
    ("神田", &["カンダ"]),
    ("富士", &["フジ"]),
    ("北沢", &["キタザワ"]),
    // Common prefix words of company names.
    ("中央", &["チュウオウ"]),
    ("大東", &["ダイトウ"]),
    ("日東", &["ニットウ"]),
    ("東洋", &["トウヨウ"]),
    ("昭和", &["ショウワ"]),
    ("大正", &["タイショウ"]),
    ("太平", &["タイヘイ"]),
    ("平和", &["ヘイワ"]),
    ("共栄", &["キョウエイ"]),
    ("協和", &["キョウワ"]),
    ("三協", &["サンキョウ"]),
    ("三和", &["サンワ"]),
    ("大和", &["ダイワ", "ヤマト"]),
    ("大成", &["タイセイ"]),
    ("太陽", &["タイヨウ"]),
    ("朝日", &["アサヒ"]),
    ("旭", &["アサヒ"]),
    ("丸一", &["マルイチ"]),
    ("第一", &["ダイイチ"]),
    ("日立", &["ヒタチ"]),
    // Common family names.
    ("高橋", &["タカハシ"]),
    ("山崎", &["ヤマザキ", "ヤマサキ"]),
    ("斎藤", &["サイトウ"]),
    ("渡辺", &["ワタナベ"]),
    ("桜井", &["サクライ"]),
    ("菊池", &["キクチ"]),
    ("佐藤", &["サトウ"]),
    ("鈴木", &["スズキ"]),
    ("田中", &["タナカ"]),
    ("伊藤", &["イトウ"]),
    ("山本", &["ヤマモト"]),
    ("中村", &["ナカムラ"]),
    ("小林", &["コバヤシ"]),
    ("加藤", &["カトウ"]),
    ("吉田", &["ヨシダ"]),
    ("山田", &["ヤマダ"]),
    ("佐々木", &["ササキ"]),
    ("山口", &["ヤマグチ"]),
    ("松本", &["マツモト"]),
    ("井上", &["イノウエ"]),
    ("木村", &["キムラ"]),
    ("清水", &["シミズ"]),
    ("池田", &["イケダ"]),
    ("橋本", &["ハシモト"]),
    ("石川", &["イシカワ"]),
    ("前田", &["マエダ"]),
    ("藤田", &["フジタ"]),
    ("岡田", &["オカダ"]),
    ("後藤", &["ゴトウ"]),
    ("村上", &["ムラカミ"]),
    ("近藤", &["コンドウ"]),
    ("坂本", &["サカモト"]),
    ("遠藤", &["エンドウ"]),
    ("青木", &["アオキ"]),
    ("西村", &["ニシムラ"]),
    ("福田", &["フクダ"]),
    ("太田", &["オオタ"]),
    ("三浦", &["ミウラ"]),
    ("藤井", &["フジイ"]),
    ("岡本", &["オカモト"]),
    ("松田", &["マツダ"]),
    ("中川", &["ナカガワ"]),
    ("中野", &["ナカノ"]),
    ("原田", &["ハラダ"]),
    ("小野", &["オノ"]),
    ("竹内", &["タケウチ"]),
    ("金子", &["カネコ"]),
    ("和田", &["ワダ"]),
    ("中山", &["ナカヤマ"]),
    ("石田", &["イシダ"]),
    ("上田", &["ウエダ"]),
    ("森田", &["モリタ"]),
    ("柴田", &["シバタ"]),
    ("酒井", &["サカイ"]),
    ("工藤", &["クドウ"]),
    ("横山", &["ヨコヤマ"]),
    ("宮崎", &["ミヤザキ"]),
    ("宮本", &["ミヤモト"]),
    ("内田", &["ウチダ"]),
    ("高木", &["タカギ"]),
    ("安藤", &["アンドウ"]),
    ("島田", &["シマダ"]),
    ("谷口", &["タニグチ"]),
    ("大野", &["オオノ"]),
    ("丸山", &["マルヤマ"]),
    ("今井", &["イマイ"]),
    ("藤原", &["フジワラ"]),
    ("小川", &["オガワ"]),
];

/// At most this many alternative readings per name (combinations of multi-reading words).
pub const MAX_READINGS: usize = 4;

/// Longest dictionary word, in characters.
fn max_word_chars() -> usize {
    WORDS.iter().map(|(w, _)| w.chars().count()).max().unwrap_or(1)
}

fn is_katakana(c: char) -> bool {
    ('\u{30A1}'..='\u{30FA}').contains(&c) || c == 'ー'
}

/// Katakana readings of a folded company core (kanji in standard form, kana already katakana, no legal form,
/// no spaces). Empty unless every character is covered (dictionary word or katakana). At most
/// [`MAX_READINGS`] readings, in a deterministic order. Names without kanji return empty (they have a written
/// reading already).
pub fn readings(core: &str) -> Vec<String> {
    let c: Vec<char> = core.chars().collect();
    if c.is_empty() || c.iter().all(|&x| is_katakana(x) || x.is_ascii_alphanumeric()) {
        return Vec::new();
    }
    let longest = max_word_chars();
    let mut out: Vec<String> = vec![String::new()];
    let mut i = 0;
    while i < c.len() {
        if is_katakana(c[i]) {
            let run: String = c[i..].iter().take_while(|&&x| is_katakana(x)).collect();
            i += run.chars().count();
            for r in &mut out {
                r.push_str(&run);
            }
            continue;
        }
        // Greedy longest match (deterministic; company words rarely need backtracking).
        let hit = (1..=longest.min(c.len() - i)).rev().find_map(|len| {
            let w: String = c[i..i + len].iter().collect();
            WORDS.iter().find(|(k, _)| *k == w).map(|(_, r)| (len, *r))
        });
        let Some((len, rs)) = hit else {
            return Vec::new();
        };
        i += len;
        let mut next = Vec::with_capacity(out.len() * rs.len());
        for prefix in &out {
            for r in rs {
                if next.len() < MAX_READINGS {
                    next.push(format!("{prefix}{r}"));
                }
            }
        }
        out = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_coverage_only() {
        assert_eq!(readings("東海精機"), vec!["トウカイセイキ"]);
        assert_eq!(readings("名古屋製作所"), vec!["ナゴヤセイサクショ"]);
        assert_eq!(readings("日本工業"), vec!["ニッポンコウギョウ", "ニホンコウギョウ"]);
        assert_eq!(readings("東海セイキ"), vec!["トウカイセイキ"]);
        assert!(readings("甲府精機").is_empty(), "unknown kanji → no reading");
        assert!(readings("トウカイセイキ").is_empty(), "already a written reading");
        assert!(readings("").is_empty());
    }

    #[test]
    fn longest_word_wins_and_alternatives_are_capped() {
        assert_eq!(readings("重工業"), vec!["ジュウコウギョウ"]);
        assert!(readings("日本大和日本大和").len() <= MAX_READINGS);
    }

    #[test]
    fn dictionary_is_well_formed() {
        for (w, rs) in WORDS {
            assert!(!w.is_empty() && !rs.is_empty(), "{w}");
            assert!(rs.iter().all(|r| r.chars().all(is_katakana)), "{w}: {rs:?}");
        }
        let mut ws: Vec<&str> = WORDS.iter().map(|(w, _)| *w).collect();
        ws.sort();
        let n = ws.len();
        ws.dedup();
        assert_eq!(ws.len(), n, "duplicate dictionary word");
    }
}
