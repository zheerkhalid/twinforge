//! Fuzzy text matching for music tags: tidy the text, then measure how alike
//! two tidied texts are.

/// Folds the accented Latin letters that turn up in tags to plain letters.
fn fold(c: char) -> &'static str {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ĝ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ľ' | 'ĺ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' | 'ŕ' => "r",
        'ś' | 'š' | 'ş' | 'ŝ' => "s",
        'ß' => "ss",
        'ť' | 'ţ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => "",
    }
}

/// Tidies letters of the Arabic script (Arabic, Kurdish, Persian, Urdu) so
/// that the same word typed on different keyboards compares equal.
/// `None`: not such a character. `Some(None)`: a mark that is dropped.
fn fold_arabic(c: char) -> Option<Option<char>> {
    Some(match c {
        // Vowel marks, the stretching line and invisible joiners.
        '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0640}' | '\u{200C}'..='\u{200F}' => None,
        // The three spellings of "ya" and the two of "kaf".
        '\u{064A}' | '\u{0649}' | '\u{06CC}' => Some('\u{06CC}'),
        '\u{0643}' | '\u{06A9}' => Some('\u{06A9}'),
        // "ta marbuta", the Kurdish "ae" and the two-eyed "he" are all typed for "he".
        '\u{0629}' | '\u{06D5}' | '\u{06BE}' | '\u{0647}' => Some('\u{0647}'),
        // "alef" with a hamza or madda.
        '\u{0623}' | '\u{0625}' | '\u{0622}' | '\u{0671}' => Some('\u{0627}'),
        '\u{0624}' => Some('\u{0648}'),
        // Arabic and Persian digits.
        '\u{0660}'..='\u{0669}' => char::from_digit(c as u32 - 0x0660, 10),
        '\u{06F0}'..='\u{06F9}' => char::from_digit(c as u32 - 0x06F0, 10),
        _ => return None,
    })
}

/// Removes everything inside (...), [...] and {...}.
fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Tidies a title, artist or album for comparison: lower case, accents folded,
/// bracketed extras such as "(Remastered 2011)" or "[Live]" and "feat. X"
/// dropped, punctuation turned into spaces, "The" at either end removed.
pub fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let stripped = strip_brackets(&lower);
    // If the whole text was in brackets, keep it rather than end up with nothing.
    let source = if stripped.trim().is_empty() {
        lower.replace(['(', ')', '[', ']', '{', '}'], " ")
    } else {
        stripped
    };
    let mut plain = String::with_capacity(source.len());
    for c in source.chars() {
        if let Some(arabic) = fold_arabic(c) {
            plain.extend(arabic);
            continue;
        }
        let folded = fold(c);
        if !folded.is_empty() {
            plain.push_str(folded);
        } else if c == '&' {
            plain.push_str(" and ");
        } else if c.is_alphanumeric() {
            plain.push(c);
        } else if c == '\'' || c == '\u{2019}' {
            // don't -> dont
        } else {
            plain.push(' ');
        }
    }
    let mut words: Vec<&str> = plain.split_whitespace().collect();
    // "feat. someone" and everything after it is an extra.
    if let Some(i) = words
        .iter()
        .position(|w| matches!(*w, "feat" | "ft" | "featuring"))
    {
        if i > 0 {
            words.truncate(i);
        }
    }
    if words.len() > 1 && words[0] == "the" {
        words.remove(0);
    }
    if words.len() > 1 && words[words.len() - 1] == "the" {
        words.pop();
    }
    words.join(" ")
}

/// Edit distance between two character slices, giving up (returning
/// `limit + 1`) as soon as it is certain to exceed `limit`.
fn edit_distance(a: &[char], b: &[char], limit: usize) -> usize {
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        let mut row_min = cur[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            row_min = row_min.min(cur[j + 1]);
        }
        if row_min > limit {
            return limit + 1;
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn ratio(a: &str, b: &str) -> u8 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 0;
    }
    let d = edit_distance(&a, &b, longest);
    (100.0 * (1.0 - d as f64 / longest as f64)).round() as u8
}

/// How alike two already normalised texts are, 0..=100. Word order counts for
/// less: "low sun tidewater" and "tidewater low sun" are the same.
pub fn similarity_normalized(a: &str, b: &str) -> u8 {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    if a == b {
        return 100;
    }
    // "Part 1" and "Part 2" are nearly the same text but not the same thing:
    // when the numbers in the two texts differ, the score is zero.
    let numbers = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .map(|p| p.trim_start_matches('0').to_string())
            .collect()
    };
    if numbers(a) != numbers(b) {
        return 0;
    }
    let direct = ratio(a, b);
    if direct >= 100 || (!a.contains(' ') && !b.contains(' ')) {
        return direct;
    }
    let sorted = |s: &str| {
        let mut w: Vec<&str> = s.split(' ').collect();
        w.sort_unstable();
        w.join(" ")
    };
    direct.max(ratio(&sorted(a), &sorted(b)))
}

/// How alike two raw texts are after tidying, 0..=100. Empty text never
/// matches anything, not even other empty text.
pub fn similarity(a: &str, b: &str) -> u8 {
    similarity_normalized(&normalize(a), &normalize(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidying() {
        assert_eq!(normalize("  Low  Sun (Remastered 2011) "), "low sun");
        assert_eq!(normalize("Low Sun [Live] {bonus}"), "low sun");
        assert_eq!(normalize("Low Sun feat. Mira Vale"), "low sun");
        assert_eq!(normalize("Low Sun ft Mira Vale"), "low sun");
        assert_eq!(normalize("The Paper Kites"), "paper kites");
        assert_eq!(normalize("Paper Kites, The"), "paper kites");
        assert_eq!(normalize("The"), "the");
        assert_eq!(normalize("Señor Café: Über-Naïve"), "senor cafe uber naive");
        assert_eq!(normalize("Salt & Stone"), "salt and stone");
        assert_eq!(normalize("Don't Stop"), "dont stop");
        assert_eq!(normalize("(Untitled)"), "untitled");
        assert_eq!(normalize("03 - Low_Sun"), "03 low sun");
        assert_eq!(normalize("!!!"), "");
        assert_eq!(normalize("Straße"), "strasse");
    }

    #[test]
    fn alike_texts_score_high() {
        assert_eq!(similarity("Low Sun", "low sun"), 100);
        assert_eq!(similarity("Low Sun", "Low Sun (Remastered)"), 100);
        assert_eq!(similarity("The Paper Kites", "Paper Kites, The"), 100);
        assert_eq!(similarity("Tidewater - Low Sun", "Low Sun, Tidewater"), 100);
        assert!(similarity("Glass Morning", "Glass Mornin") >= 85);
        assert!(similarity("Northern Lights", "Northen Lights") >= 85);
        assert!(similarity("Café del Mar", "Cafe Del Mar") == 100);
    }

    #[test]
    fn different_texts_score_low() {
        assert!(similarity("Low Sun", "Glass Morning") < 50);
        assert!(similarity("Low Sun", "Low Tide") < 85);
        assert_eq!(similarity("Part 1", "Part 2"), 0);
        assert_eq!(similarity("Part 01", "part 1"), 86);
        assert_eq!(similarity("", ""), 0);
        assert_eq!(similarity("Low Sun", ""), 0);
        assert_eq!(similarity("???", "!!!"), 0);
    }

    #[test]
    fn edit_distance_basics() {
        let d = |a: &str, b: &str| {
            edit_distance(
                &a.chars().collect::<Vec<_>>(),
                &b.chars().collect::<Vec<_>>(),
                100,
            )
        };
        assert_eq!(d("kitten", "sitting"), 3);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("same", "same"), 0);
        // The early exit reports "more than the limit".
        assert_eq!(edit_distance(&['a'; 10], &['b'; 10], 3), 4);
    }

    #[test]
    fn arabic_script_spellings_compare_equal() {
        // Kurdish "Kurdistan" with an Arabic kaf and with the Kurdish/Persian one.
        assert_eq!(
            normalize("\u{643}\u{648}\u{631}\u{62f}\u{633}\u{62a}\u{627}\u{646}"),
            normalize("\u{6a9}\u{648}\u{631}\u{62f}\u{633}\u{62a}\u{627}\u{646}")
        );
        // Arabic "ya" and Persian/Kurdish "ya"; vowel marks and the stretching line are ignored.
        assert_eq!(
            normalize("\u{62d}\u{64e}\u{628}\u{640}\u{64a}\u{628}\u{64a}"),
            normalize("\u{62d}\u{628}\u{6cc}\u{628}\u{6cc}")
        );
        // The marks do not split a word in two.
        assert!(!normalize("\u{62d}\u{64e}\u{628}\u{64a}\u{628}\u{64a}").contains(' '));
        // Arabic digits are digits: track 2 is not track 3.
        assert_eq!(normalize("\u{661}\u{662}"), "12");
        assert_eq!(
            similarity_normalized(
                &normalize("\u{628}\u{634}\u{6cc} \u{662}"),
                &normalize("\u{628}\u{634}\u{6cc} \u{663}")
            ),
            0
        );
        // Different words stay different.
        assert!(
            similarity_normalized(
                &normalize("\u{6af}\u{6c6}\u{631}\u{627}\u{646}\u{6cc}"),
                &normalize("\u{62a}\u{642}\u{631}\u{64a}\u{631}")
            ) < 60
        );
    }
}
