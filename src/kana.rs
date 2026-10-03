//! 検索用のかな正規化（カタカナ・半角カナ → ひらがな）
//!
//! aviutl2-rs の examples/scripts-search-plugin（Rusty Scripts Search）の lib.rs から写したもの。
//! Copyright (c) 2025 Nanashi. <sevenc7c.com> / MIT License
//! https://github.com/sevenc-nanashi/aviutl2-rs

pub fn normalize_kana_for_search(input: &str) -> String {
    let (normalized, _) = normalize_kana_for_search_with_map(input);
    normalized
}

/// 正規化後の文字 i が、元の文字列の何文字目から何文字目に当たるかの対応を返す
/// （半角カナの濁点は 2 文字が 1 文字になる）。
pub fn normalize_kana_for_search_with_map(input: &str) -> (String, Vec<(usize, usize)>) {
    if input.is_empty() {
        return (String::new(), Vec::new());
    }
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(chars.len());
    let mut map = Vec::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        let start = index;
        let c = chars[index];
        if is_hiragana(c) {
            output.push(c);
            map.push((start, start));
            index += 1;
            continue;
        }
        if is_katakana(c) {
            output.push(katakana_to_hiragana(c));
            map.push((start, start));
            index += 1;
            continue;
        }
        if let Some(mut kata) = halfwidth_to_fullwidth_katakana(c) {
            let mut end = start;
            if index + 1 < chars.len() {
                let next = chars[index + 1];
                if (next == 'ﾞ' || next == 'ﾟ')
                    && let Some(combined) = apply_diacritic(kata, next)
                {
                    kata = combined;
                    end = index + 1;
                    index += 1;
                }
            }
            if is_katakana(kata) {
                output.push(katakana_to_hiragana(kata));
            } else {
                output.push(kata);
            }
            map.push((start, end));
            index += 1;
            continue;
        }
        output.push(c);
        map.push((start, start));
        index += 1;
    }
    (output, map)
}

fn is_hiragana(c: char) -> bool {
    ('\u{3041}'..='\u{3096}').contains(&c)
}

fn is_katakana(c: char) -> bool {
    ('\u{30A1}'..='\u{30F6}').contains(&c)
}

fn katakana_to_hiragana(c: char) -> char {
    let code = u32::from(c) - 0x60;
    char::from_u32(code).unwrap_or(c)
}

fn halfwidth_to_fullwidth_katakana(c: char) -> Option<char> {
    let mapped = match c {
        'ｦ' => 'ヲ',
        'ｧ' => 'ァ',
        'ｨ' => 'ィ',
        'ｩ' => 'ゥ',
        'ｪ' => 'ェ',
        'ｫ' => 'ォ',
        'ｬ' => 'ャ',
        'ｭ' => 'ュ',
        'ｮ' => 'ョ',
        'ｯ' => 'ッ',
        'ｰ' => 'ー',
        'ｱ' => 'ア',
        'ｲ' => 'イ',
        'ｳ' => 'ウ',
        'ｴ' => 'エ',
        'ｵ' => 'オ',
        'ｶ' => 'カ',
        'ｷ' => 'キ',
        'ｸ' => 'ク',
        'ｹ' => 'ケ',
        'ｺ' => 'コ',
        'ｻ' => 'サ',
        'ｼ' => 'シ',
        'ｽ' => 'ス',
        'ｾ' => 'セ',
        'ｿ' => 'ソ',
        'ﾀ' => 'タ',
        'ﾁ' => 'チ',
        'ﾂ' => 'ツ',
        'ﾃ' => 'テ',
        'ﾄ' => 'ト',
        'ﾅ' => 'ナ',
        'ﾆ' => 'ニ',
        'ﾇ' => 'ヌ',
        'ﾈ' => 'ネ',
        'ﾉ' => 'ノ',
        'ﾊ' => 'ハ',
        'ﾋ' => 'ヒ',
        'ﾌ' => 'フ',
        'ﾍ' => 'ヘ',
        'ﾎ' => 'ホ',
        'ﾏ' => 'マ',
        'ﾐ' => 'ミ',
        'ﾑ' => 'ム',
        'ﾒ' => 'メ',
        'ﾓ' => 'モ',
        'ﾔ' => 'ヤ',
        'ﾕ' => 'ユ',
        'ﾖ' => 'ヨ',
        'ﾗ' => 'ラ',
        'ﾘ' => 'リ',
        'ﾙ' => 'ル',
        'ﾚ' => 'レ',
        'ﾛ' => 'ロ',
        'ﾜ' => 'ワ',
        'ﾝ' => 'ン',
        _ => return None,
    };
    Some(mapped)
}

fn apply_diacritic(base: char, mark: char) -> Option<char> {
    let mapped = match (base, mark) {
        ('ウ', 'ﾞ') => 'ヴ',
        ('カ', 'ﾞ') => 'ガ',
        ('キ', 'ﾞ') => 'ギ',
        ('ク', 'ﾞ') => 'グ',
        ('ケ', 'ﾞ') => 'ゲ',
        ('コ', 'ﾞ') => 'ゴ',
        ('サ', 'ﾞ') => 'ザ',
        ('シ', 'ﾞ') => 'ジ',
        ('ス', 'ﾞ') => 'ズ',
        ('セ', 'ﾞ') => 'ゼ',
        ('ソ', 'ﾞ') => 'ゾ',
        ('タ', 'ﾞ') => 'ダ',
        ('チ', 'ﾞ') => 'ヂ',
        ('ツ', 'ﾞ') => 'ヅ',
        ('テ', 'ﾞ') => 'デ',
        ('ト', 'ﾞ') => 'ド',
        ('ハ', 'ﾞ') => 'バ',
        ('ヒ', 'ﾞ') => 'ビ',
        ('フ', 'ﾞ') => 'ブ',
        ('ヘ', 'ﾞ') => 'ベ',
        ('ホ', 'ﾞ') => 'ボ',
        ('ワ', 'ﾞ') => 'ヷ',
        ('ヰ', 'ﾞ') => 'ヸ',
        ('ヱ', 'ﾞ') => 'ヹ',
        ('ヲ', 'ﾞ') => 'ヺ',
        ('ハ', 'ﾟ') => 'パ',
        ('ヒ', 'ﾟ') => 'ピ',
        ('フ', 'ﾟ') => 'プ',
        ('ヘ', 'ﾟ') => 'ペ',
        ('ホ', 'ﾟ') => 'ポ',
        _ => return None,
    };
    Some(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_katakana_and_halfwidth() {
        let input = "あいうえおアイウエオカキクケコｱｲｳｴｵｶﾞｷﾞｸﾞｹﾞｺﾞ";
        let expected = "あいうえおあいうえおかきくけこあいうえおがぎぐげご";
        assert_eq!(normalize_kana_for_search(input), expected);
    }

    #[test]
    fn map_points_back_to_both_chars_of_dakuten() {
        let (s, map) = normalize_kana_for_search_with_map("ｶﾞa");
        assert_eq!(s, "がa");
        assert_eq!(map, vec![(0, 1), (2, 2)]);
    }
}
