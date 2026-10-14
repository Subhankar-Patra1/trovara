//! Text the way keyword search keeps it and matches it, so a word is found however it was typed or extracted
//! (TRD §7).

use std::ops::Range;

use unicode_general_category::{get_general_category, GeneralCategory as G};
use unicode_normalization::UnicodeNormalization;

/// NFKC (Arabic presentation forms and ligatures become plain letters, full-width becomes ASCII, Bengali's two
/// spellings of "ো" become one), invisible formatting characters dropped (Bengali's zero-width joiners, direction
/// marks), and Arabic the way people type it: no tatweel, no short-vowel marks, one alef.
pub fn normalized(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.nfkc() {
        match c as u32 {
            0x200B => out.push(' '), // zero-width space: a word break (Thai, Khmer)
            0x00AD
            | 0x061C
            | 0x200C..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x2069
            | 0xFEFF => {}
            0x0640 | 0x064B..=0x065F | 0x0670 => {} // tatweel; harakat and the dagger alef
            0x0622 | 0x0623 | 0x0625 | 0x0671 => out.push('\u{0627}'), // alef with hamza or madda, wasla → alef
            _ => out.push(c),
        }
    }
    out
}

const INDIC: Range<u32> = 0x0900..0x0E00;
const ARABIC: Range<u32> = 0x0600..0x0700;

/// True for words of scripts that attach endings to words (Bengali, Hindi, Arabic): keyword search matches them as
/// prefixes, so "কুমির" finds "কুমিরও" and "مدرس" finds "مدرسة".
pub fn inflects(word: &str) -> bool {
    word.chars()
        .any(|c| INDIC.contains(&(c as u32)) || ARABIC.contains(&(c as u32)))
}

/// Letters and marks (Bengali vowel signs and viramas, Arabic harakat), as Foundation's `CharacterSet.letters`.
pub fn is_letter(c: char) -> bool {
    matches!(
        get_general_category(c),
        G::UppercaseLetter
            | G::LowercaseLetter
            | G::TitlecaseLetter
            | G::ModifierLetter
            | G::OtherLetter
            | G::NonspacingMark
            | G::SpacingMark
            | G::EnclosingMark
    )
}

/// Letters, marks and numbers: what a word is made of.
pub fn is_word_char(c: char) -> bool {
    is_letter(c)
        || matches!(
            get_general_category(c),
            G::DecimalNumber | G::LetterNumber | G::OtherNumber
        )
}

/// Whether text says something in words: at least a fifth of what isn't space is letters. A data dump or a table of
/// numbers isn't, and gets keyword search only: a meaning vector of digits matches nothing anyone would describe.
pub fn has_words(text: &str) -> bool {
    let (mut letters, mut others) = (0usize, 0usize);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        if is_letter(c) {
            letters += 1;
        } else {
            others += 1;
        }
    }
    letters > 0 && letters * 4 >= others
}

/// One character as keyword search compares it: normalized, lowercased, without Latin accents (FTS5's
/// `remove_diacritics 2` does the same to what it stores).
fn fold(c: char) -> String {
    let lower = normalized(&c.to_string()).to_lowercase();
    lower
        .nfd()
        .filter(|c| !(0x0300..=0x036F).contains(&(*c as u32)))
        .nfc()
        .collect()
}

/// Where `word` is in `text` as keyword search would find it (normalized, ignoring case and accents), as byte ranges of
/// the text as written: whole words, and for a word of a language that attaches endings, the whole word it starts
/// ("কুমির" marks all of "কুমিরের"). For marking matches on what's shown.
pub fn matches(word: &str, text: &str) -> Vec<Range<usize>> {
    let needle: String = word.chars().map(fold).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // The folded text, and for each of its bytes the original character's byte range.
    let mut folded = String::with_capacity(text.len());
    let mut origin: Vec<(usize, usize)> = Vec::with_capacity(text.len());
    for (start, c) in text.char_indices() {
        let piece = fold(c);
        origin.extend(std::iter::repeat_n(
            (start, start + c.len_utf8()),
            piece.len(),
        ));
        folded.push_str(&piece);
    }
    let prefix = inflects(word);
    let mut found: Vec<Range<usize>> = Vec::new();
    let mut from = 0;
    while found.len() < 500 {
        let Some(at) = folded[from..].find(&needle).map(|at| at + from) else {
            break;
        };
        from = at + needle.len();
        let mut range = origin[at].0..origin[at + needle.len() - 1].1;
        if text[..range.start]
            .chars()
            .next_back()
            .is_some_and(is_word_char)
        {
            continue;
        }
        if text[range.end..].chars().next().is_some_and(is_word_char) {
            if !prefix {
                continue;
            }
            let rest = &text[range.end..];
            let extra: usize = rest
                .chars()
                .take_while(|&c| is_word_char(c))
                .map(char::len_utf8)
                .sum();
            range.end += extra;
        }
        if found.last().is_some_and(|last| last.end > range.start) {
            continue;
        }
        found.push(range);
    }
    found
}

/// One line from `text` around the first of `terms` it has, the terms marked « » as SQLite's snippet() marks them:
/// the excerpt of a keyword match, from the text as written. None when none of the terms is in it.
pub fn snippet(text: &str, terms: &[String], words: usize) -> Option<String> {
    let mut marks: Vec<Range<usize>> = terms.iter().flat_map(|term| matches(term, text)).collect();
    marks.sort_by_key(|mark| mark.start);
    let first = marks.first()?.clone();
    // Work in characters: byte offsets of each character, plus the end.
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let chars: Vec<char> = text.chars().collect();
    let index_of = |byte: usize| offsets.binary_search(&byte).unwrap_or_else(|i| i);
    let (first_start, first_end) = (index_of(first.start), index_of(first.end));
    let ws = |i: usize| chars[i].is_whitespace();

    // About `words` words, starting a few before the first match.
    let (mut start, mut before) = (first_start, 0);
    while start > 0 && before < words / 3 {
        start -= 1;
        if ws(start) && start < first_start && !ws(start + 1) {
            before += 1;
        }
    }
    if start > 0 || (start < chars.len() && ws(start)) {
        start += 1;
    }
    let (mut end, mut after) = (first_end, 0);
    while end < chars.len() && after < words.saturating_sub(before) {
        if ws(end) {
            after += 1;
        }
        end += 1;
    }
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut cursor = start;
    for mark in &marks {
        let (a, b) = (index_of(mark.start), index_of(mark.end));
        if a >= start && b <= end && a >= cursor {
            out.push_str(&text[offsets[cursor]..offsets[a]]);
            out.push('«');
            out.push_str(&text[offsets[a]..offsets[b]]);
            out.push('»');
            cursor = b;
        }
    }
    out.push_str(&text[offsets[cursor]..offsets[end.max(cursor)]]);
    if end < chars.len() {
        out.push('…');
    }
    Some(collapse_whitespace(&out))
}

/// Runs of whitespace become one space; the ends are trimmed.
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line, at most `limit` characters: for result lists.
pub fn excerpt(text: &str, limit: usize) -> String {
    // Only the start is shown, so only the start is cleaned up (a page or a chunk can be thousands of characters).
    let start: String = text.chars().take(limit * 8).collect();
    let truncated = start.len() < text.len();
    let line = collapse_whitespace(&start);
    if line.chars().count() > limit || truncated {
        let mut out: String = line.chars().take(limit).collect();
        out.push('…');
        out
    } else {
        line
    }
}

/// Splits text into ~`size`-character pieces (~450 tokens) that overlap a little, breaking at whitespace.
pub fn chunks(text: &str, size: usize, overlap: usize, limit: usize) -> Vec<String> {
    // Spaces, tabs and no-break spaces collapse; three or more line breaks become two.
    let mut clean = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c == ' ' || c == '\t' || c == '\u{00A0}' {
            if !in_space {
                clean.push(' ');
            }
            in_space = true;
        } else {
            in_space = false;
            clean.push(if c == '\r' { '\n' } else { c });
        }
    }
    let clean = collapse_blank_lines(&clean);
    let chars: Vec<char> = clean.trim().chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() && out.len() < limit {
        let mut end = (start + size).min(chars.len());
        if end < chars.len() {
            if let Some(space) = (start..end).rev().find(|&i| chars[i].is_whitespace()) {
                if space - start > size / 2 {
                    end = space;
                }
            }
        }
        let chunk: String = chars[start..end]
            .iter()
            .collect::<String>()
            .trim()
            .to_string();
        if !chunk.is_empty() {
            out.push(chunk);
        }
        if end >= chars.len() {
            break;
        }
        let mut next = end.saturating_sub(overlap).max(start);
        if next <= start {
            next = end;
        }
        // Start the next chunk on a word boundary.
        while next < end && !chars[next].is_whitespace() {
            next += 1;
        }
        start = if next < end { next } else { end };
    }
    out
}

fn collapse_blank_lines(text: &str) -> String {
    // "\n\s*\n\s*\n+" → "\n\n"
    let mut out = String::with_capacity(text.len());
    let mut pending = String::new();
    let mut newlines = 0;
    for c in text.chars() {
        if c.is_whitespace() {
            if c == '\n' {
                newlines += 1;
            }
            pending.push(c);
        } else {
            if newlines >= 3 {
                out.push_str("\n\n");
            } else {
                out.push_str(&pending);
            }
            pending.clear();
            newlines = 0;
            out.push(c);
        }
    }
    out.push_str(&pending);
    out
}

/// What's wrong with the text a PDF library read from a page, when the page's text layer can't be used as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTextProblem {
    /// Bengali or Hindi read in glyph order: vowel signs ahead of their consonants, conjuncts lost ("চুক্তিপত্র" reads
    /// "চিপ"). The page is used as a picture instead, and its garbled words only for a meaning vector.
    GlyphOrder,
    /// Arabic letters lost where the font drew a lam ligature: a bare alef before a "?" or "." ("الأردن" reads
    /// "ا?ردن"). The page is read again by OCR (M4). A bare alef, not أ: "أ.د." (Prof. Dr.) is a real abbreviation.
    LostLetters,
}

impl PageTextProblem {
    pub fn check(text: &str) -> Option<PageTextProblem> {
        let (mut vowel_signs, mut misplaced, mut lost) = (0usize, 0usize, 0usize);
        let (mut previous, mut before_previous) = (' ', ' ');
        for c in text.nfc() {
            let value = c as u32;
            if is_indic_vowel_sign(value) {
                vowel_signs += 1;
                if !is_indic_consonant_or_nukta(previous as u32) {
                    misplaced += 1;
                }
            }
            if before_previous == '\u{0627}'
                && matches!(previous, '?' | '.' | '\u{FFFD}')
                && ARABIC.contains(&value)
                && c.is_alphabetic()
            {
                lost += 1;
            }
            before_previous = previous;
            previous = c;
        }
        if vowel_signs >= 10 && misplaced * 20 >= vowel_signs {
            return Some(PageTextProblem::GlyphOrder);
        }
        if lost >= 2 {
            return Some(PageTextProblem::LostLetters);
        }
        None
    }
}

/// Dependent vowel signs of Bengali and Devanagari: in a real word, each follows a consonant (or its nukta).
fn is_indic_vowel_sign(value: u32) -> bool {
    matches!(value,
        0x09BE..=0x09C4 | 0x09C7 | 0x09C8 | 0x09CB | 0x09CC | 0x09D7 | 0x09E2 | 0x09E3
        | 0x093A | 0x093B | 0x093E..=0x094C | 0x094E | 0x094F | 0x0955..=0x0957 | 0x0962 | 0x0963)
}

fn is_indic_consonant_or_nukta(value: u32) -> bool {
    matches!(value,
        0x0995..=0x09B9 | 0x09BC | 0x09CE | 0x09DC | 0x09DD | 0x09DF | 0x09F0 | 0x09F1
        | 0x0915..=0x0939 | 0x093C | 0x0958..=0x095F | 0x0979..=0x097F)
}

#[cfg(test)]
mod tests {
    #[test]
    fn garbled_page_text_is_noticed() {
        // Glyph order: the vowel sign ি (U+09BF) before its consonant, over and over.
        let glyph_order = "\u{09BF}ক \u{09BF}খ ".repeat(8);
        assert_eq!(
            PageTextProblem::check(&glyph_order),
            Some(PageTextProblem::GlyphOrder)
        );
        assert_eq!(
            PageTextProblem::check("কিছু কথা বলি, তুমি শোনো। আমরা কাল বাজারে যাব।"),
            None
        );
        assert_eq!(
            PageTextProblem::check("زرنا مدينة البتراء ا?ثرية في جنوب ا?ردن"),
            Some(PageTextProblem::LostLetters)
        );
        assert_eq!(
            PageTextProblem::check(
                "أ.د.محمد علي، أستاذ في الجامعة. د.خالد و د.سارة من شركة ذ.م.م للتجارة"
            ),
            None
        );
        assert_eq!(
            PageTextProblem::check(&"\u{F0B7} Item one of the slide\n".repeat(6)),
            None
        );
    }

    use super::*;

    fn found(word: &str, text: &str) -> Vec<String> {
        matches(word, text)
            .into_iter()
            .map(|range| text[range].to_string())
            .collect()
    }

    #[test]
    fn normalized_folds_what_is_typed_differently() {
        assert_eq!(normalized("الْمَكْتَبَةَ"), "المكتبة"); // harakat
        assert_eq!(normalized("إلى الأردن"), "الى الاردن"); // alef forms
        assert_eq!(normalized("مـــرحبا"), "مرحبا"); // tatweel
        assert_eq!(normalized("\u{FEFB}"), "\u{0644}\u{0627}"); // the lam-alef ligature → lam, alef
        assert_eq!(normalized("\u{FB01}le"), "file"); // ﬁ
        assert_eq!(normalized("র\u{200D}্যাব"), "র্যাব"); // zero-width joiner
        assert_eq!(normalized("\u{09C7}\u{09BE}"), "\u{09CB}"); // ে + া → ো
        assert_eq!(normalized("a\u{200B}b"), "a b"); // zero-width space breaks words
    }

    #[test]
    fn matches_are_found_as_keyword_search_finds_them_but_shown_as_written() {
        assert_eq!(
            found("الاردن", "زرنا مدينة البتراء الأثرية في جنوب الأردن."),
            ["الأردن"]
        );
        assert_eq!(found("المكتبة", "إِنَّ الْمَكْتَبَةَ تُغْلَقُ"), ["الْمَكْتَبَةَ"]);
        assert_eq!(found("কুমির", "একটি বড় কুমিরের ছবি"), ["কুমিরের"]); // never "কুমি"
        assert_eq!(found("day", "Sunday trip: day two"), ["day"]); // not in "Sunday"
        let turkish = "İ".repeat(60) + " son kelime";
        assert_eq!(found("kelime", &turkish), ["kelime"]);
        assert_eq!(found("cafe", "Meet at the Café Lumière"), ["Café"]);
    }

    #[test]
    fn snippets_quote_the_text_as_written() {
        let text = "قَالَ مُحَمَّدٌ إِنَّ الْمَكْتَبَةَ الْعَامَّةَ تُغْلَقُ فِي السَّاعَةِ الثَّامِنَةِ مَسَاءً يَوْمَ الْخَمِيسِ";
        let snippet = snippet(text, &["المكتبة".into()], 12).unwrap();
        assert!(snippet.contains("«الْمَكْتَبَةَ»"), "{snippet}");
        assert!(super::snippet(text, &["كلمة".into()], 12).is_none());
        let long = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen invoice INV-2093 \
                    paid fifteen sixteen seventeen eighteen nineteen twenty twentyone twentytwo";
        let s = super::snippet(long, &["invoice".into()], 12).unwrap();
        assert!(
            s.starts_with('…') && s.ends_with('…') && s.contains("«invoice»"),
            "{s}"
        );
    }

    #[test]
    fn words_and_numbers() {
        assert!(has_words("The cat sleeps on the sofa."));
        assert!(has_words("সুন্দরবনে আমরা কুমিরও দেখেছি"));
        assert!(!has_words("12.5, 13.7, 99.1, 4.0, 18.2, 77.3"));
        assert!(is_word_char('্')); // Bengali virama stays inside its word
        assert!(inflects("কুমির") && inflects("مدرسة") && !inflects("cat"));
    }

    #[test]
    fn chunks_overlap_and_break_at_spaces() {
        let text = (0..400)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let pieces = chunks(&text, 1800, 200, 100);
        assert!(pieces.len() >= 2);
        assert!(pieces.iter().all(|p| p.chars().count() <= 1800));
        assert!(pieces
            .iter()
            .all(|p| !p.starts_with(' ') && p.starts_with("word")));
        // The second chunk starts inside the end of the first.
        let tail: String = pieces[0]
            .chars()
            .rev()
            .take(220)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        assert!(tail.contains(pieces[1].split(' ').next().unwrap()));
        assert_eq!(chunks("a\n\n\n\n\nb", 1800, 200, 10), ["a\n\nb"]);
        assert_eq!(excerpt("  hello\n  world  ", 240), "hello world");
    }
}
