//! What a query's words mean to keyword search (TRD §7, §8.2): content words, FTS5 patterns, kind hints, lookups.

use std::collections::{BTreeSet, HashSet};

use crate::search_text::{inflects, is_word_char, normalized};
use crate::FileKind;

const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "of",
    "in",
    "on",
    "at",
    "to",
    "for",
    "with",
    "and",
    "or",
    "my",
    "me",
    "that",
    "this",
    "these",
    "those",
    "from",
    "about",
    "where",
    "what",
    "which",
    "who",
    "when",
    "is",
    "was",
    "are",
    "were",
    "be",
    "it",
    "its",
    "some",
    "any",
    "find",
    "show",
    "file",
    "files",
    "photo",
    "photos",
    "picture",
    "image",
    "images",
    "screenshot",
    "screenshots",
    "video",
    "videos",
    "pdf",
    "document",
    "documents",
    "recording",
    // Bengali and Arabic (normalized, as `terms` sees them)
    "এবং",
    "ও",
    "এই",
    "সেই",
    "একটি",
    "আমার",
    "থেকে",
    "জন্য",
    "সাথে",
    "মধ্যে",
    "কোথায়",
    "কোন",
    "কী",
    "কি",
    "যে",
    "ছবি",
    "স্ক্রিনশট",
    "ভিডিও",
    "ফাইল",
    "পিডিএফ",
    "في",
    "من",
    "على",
    "الى",
    "عن",
    "مع",
    "هذا",
    "هذه",
    "ذلك",
    "التي",
    "الذي",
    "اين",
    "ما",
    "صورة",
    "صور",
    "لقطة",
    "شاشة",
    "فيديو",
    "ملف",
    "مستند",
];

const KIND_WORDS: &[(&str, FileKind)] = &[
    ("screenshot", FileKind::Screenshot),
    ("screenshots", FileKind::Screenshot),
    ("photo", FileKind::Image),
    ("photos", FileKind::Image),
    ("picture", FileKind::Image),
    ("pictures", FileKind::Image),
    ("image", FileKind::Image),
    ("images", FileKind::Image),
    ("pdf", FileKind::Pdf),
    ("pdfs", FileKind::Pdf),
    ("paper", FileKind::Pdf),
    ("papers", FileKind::Pdf),
    ("document", FileKind::Doc),
    ("documents", FileKind::Doc),
    ("note", FileKind::Doc),
    ("notes", FileKind::Doc),
    ("video", FileKind::Video),
    ("videos", FileKind::Video),
    ("clip", FileKind::Video),
    ("movie", FileKind::Video),
    ("audio", FileKind::Audio),
    ("song", FileKind::Audio),
    ("podcast", FileKind::Audio),
    ("voice", FileKind::Audio),
    ("sound", FileKind::Audio),
    ("স্ক্রিনশট", FileKind::Screenshot),
    ("ছবি", FileKind::Image),
    ("পিডিএফ", FileKind::Pdf),
    ("নোট", FileKind::Doc),
    ("ভিডিও", FileKind::Video),
    ("অডিও", FileKind::Audio),
    ("গান", FileKind::Audio),
    ("রেকর্ডিং", FileKind::Audio),
    ("لقطة", FileKind::Screenshot),
    ("صورة", FileKind::Image),
    ("صور", FileKind::Image),
    ("مستند", FileKind::Doc),
    ("ملاحظات", FileKind::Doc),
    ("فيديو", FileKind::Video),
    ("مقطع", FileKind::Video),
    ("صوت", FileKind::Audio),
    ("تسجيل", FileKind::Audio),
    ("اغنية", FileKind::Audio),
];

/// The query's words, normalized and lowercased, split where a word ends.
fn words(text: &str) -> Vec<String> {
    normalized(text)
        .to_lowercase()
        .split(|c: char| !is_word_char(c))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The content words of a query, normalized as keyword search keeps its text. Kind words ("screenshot", "video") and
/// stopwords are dropped: they describe what to look for, not what's in it. Words of two characters or more count; a
/// Bengali letter with its vowel sign ("মা") is two.
pub fn terms(text: &str) -> Vec<String> {
    let stopwords: HashSet<&str> = STOPWORDS.iter().copied().collect();
    let mut seen = HashSet::new();
    words(text)
        .into_iter()
        .filter(|word| {
            word.chars().count() >= 2
                && !stopwords.contains(word.as_str())
                && seen.insert(word.clone())
        })
        .collect()
}

/// FTS5 patterns for the words: quoted, and as prefixes for the last word while it's being typed and for words of
/// languages that attach endings. `Searcher::search` makes a word that's in no file a prefix too.
pub fn patterns(terms: &[String], prefix_last: bool) -> Vec<String> {
    terms
        .iter()
        .enumerate()
        .map(|(index, term)| {
            let prefix = (prefix_last && index + 1 == terms.len()) || inflects(term);
            format!("\"{term}\"{}", if prefix { "*" } else { "" })
        })
        .collect()
}

/// The query's words as an FTS5 OR-query.
pub fn fts_query(text: &str, prefix_last: bool) -> Option<String> {
    let terms = terms(text);
    (!terms.is_empty()).then(|| patterns(&terms, prefix_last).join(" OR "))
}

/// Kinds the query asks for ("…in a video", "screenshot of…", "ভিডিওতে"): a soft boost, not a filter. Bengali attaches
/// case endings, so a Bengali kind word also counts at the start of a longer word.
pub fn kind_hints(text: &str) -> BTreeSet<FileKind> {
    words(text)
        .iter()
        .filter_map(|word| {
            KIND_WORDS
                .iter()
                .find(|(key, _)| key == word)
                .map(|(_, kind)| *kind)
                .or_else(|| {
                    KIND_WORDS
                        .iter()
                        .find(|(key, _)| {
                            inflects(key) && key.chars().count() >= 3 && word.starts_with(key)
                        })
                        .map(|(_, kind)| *kind)
                })
        })
        .collect()
}

/// Codes and IDs ("K7Q2LM", "TRX50") and long numbers ("4815162342") mean nothing to an embedding but everything to
/// keyword search: a query made of them weighs words double and meaning half.
pub fn is_lookup(terms: &[String]) -> bool {
    !terms.is_empty()
        && terms.iter().all(|term| {
            term.chars().any(|c| c.is_numeric())
                && (term.chars().any(char::is_alphabetic) || term.chars().count() >= 4)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_words_of_languages_with_endings_are_prefixes() {
        assert_eq!(fts_query("কুমির", false).unwrap(), "\"কুমির\"*");
        assert_eq!(
            fts_query("مدرسة الأطفال", false).unwrap(),
            "\"مدرسة\"* OR \"الاطفال\"*"
        );
        assert_eq!(fts_query("মা", false).unwrap(), "\"মা\"*"); // a letter and its vowel sign: two, so it counts
        assert_eq!(
            fts_query("payment error", false).unwrap(),
            "\"payment\" OR \"error\""
        );
        assert_eq!(
            fts_query("payment err", true).unwrap(),
            "\"payment\" OR \"err\"*"
        );
        assert_eq!(
            kind_hints("জেব্রার ভিডিওতে").into_iter().collect::<Vec<_>>(),
            [FileKind::Video]
        );
        assert_eq!(
            kind_hints("صورة حمار وحشي").into_iter().collect::<Vec<_>>(),
            [FileKind::Image]
        );
        assert_eq!(
            kind_hints("zebra in a video")
                .into_iter()
                .collect::<Vec<_>>(),
            [FileKind::Video]
        );
    }

    #[test]
    fn stopwords_and_kind_words_are_not_terms() {
        assert_eq!(terms("a photo of the dog on the beach"), ["dog", "beach"]);
        assert_eq!(terms("Dog dog DOG"), ["dog"]);
        assert!(fts_query("the of a", false).is_none());
    }

    #[test]
    fn lookups() {
        assert!(is_lookup(&terms("INV20931")));
        assert!(is_lookup(&terms("K7Q2LM TRX50")));
        // "INV-20931" splits into "inv" and "20931", as FTS5 splits it: "inv" has no digit, so meaning still counts.
        assert!(!is_lookup(&terms("INV-20931")));
        assert!(is_lookup(&terms("4815162342")));
        assert!(!is_lookup(&terms("invoice 42")));
        assert!(!is_lookup(&terms("dog beach")));
    }
}
