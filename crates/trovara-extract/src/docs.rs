//! Documents as text for search (TRD §6.4): plain text directly, HTML by stripping tags, docx and odt from the XML in
//! their zip, rtf stripped of its control words, and legacy Word (.doc, 97–2003) from its piece table.

use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::text::read_text;
use crate::{ExtractError, Result};

/// The text of a document file, by its extension.
pub fn text_of(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match ext.as_str() {
        "docx" => docx(path),
        "odt" => odt(path),
        "rtf" => Ok(rtf(&read_text(path)?)),
        "doc" => legacy_word(path),
        "html" | "htm" | "xhtml" => Ok(plain_text_from_html(&read_text(path)?)),
        "xlsx" | "xlsm" => crate::office::xlsx(path),
        "pptx" => crate::office::pptx(path),
        "epub" => crate::office::epub(path),
        "eml" => crate::office::eml(path),
        "msg" => crate::office::msg(path),
        _ => read_text(path),
    }
}

static SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script\b.*?</script\s*>").unwrap());
static STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style\b.*?</style\s*>").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static HEAD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<head\b.*?</head\s*>").unwrap());
static BLOCK_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<(br|/p|/div|/li|/h[1-6]|/tr|/section|/article)\b[^>]*>").unwrap()
});
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static NUMERIC_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").unwrap());
static LINE_BREAKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\n\s*").unwrap());

/// Good enough for search: drops scripts, styles and comments, turns block ends into line breaks, strips the
/// remaining tags, and decodes common entities.
pub fn plain_text_from_html(html: &str) -> String {
    let mut text = html.to_string();
    for pattern in [&*SCRIPT, &*STYLE, &*COMMENT, &*HEAD] {
        text = pattern.replace_all(&text, " ").into_owned();
    }
    text = BLOCK_END.replace_all(&text, "\n").into_owned();
    text = TAG.replace_all(&text, " ").into_owned();
    for (entity, character) in [
        ("&nbsp;", " "),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&mdash;", "—"),
        ("&ndash;", "–"),
        ("&hellip;", "…"),
    ] {
        text = text.replace(entity, character);
    }
    text = NUMERIC_ENTITY
        .replace_all(&text, |caps: &regex::Captures| {
            let code = &caps[1];
            let value = match code.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => code.parse().ok(),
            };
            value
                .and_then(char::from_u32)
                .map_or(" ".to_string(), String::from)
        })
        .into_owned();
    // &amp; last, so "&amp;lt;" stays "&lt;" as written.
    text = text.replace("&amp;", "&");
    let text = SPACES.replace_all(&text, " ");
    LINE_BREAKS.replace_all(&text, "\n").trim().to_string()
}

/// Word-processing XML as the little HTML `plain_text_from_html` reads: paragraph ends become `</p>`, breaks `<br>`,
/// tabs and spaces a space; every other tag is stripped there.
pub(crate) fn as_html(xml: &str, paragraphs: &[&str], breaks: &[&str], spaces: &[&str]) -> String {
    let mut text = xml.to_string();
    for tag in paragraphs {
        text = text.replace(&format!("</{tag}>"), "</p>");
    }
    for tag in breaks {
        let pattern = Regex::new(&format!(r"<{}\b[^>]*/?>", regex::escape(tag))).unwrap();
        text = pattern.replace_all(&text, "<br>").into_owned();
    }
    for tag in spaces {
        let pattern = Regex::new(&format!(r"<{}\b[^>]*/?>", regex::escape(tag))).unwrap();
        text = pattern.replace_all(&text, " ").into_owned();
    }
    text
}

/// One entry of a zip file, as text (None when it isn't there).
pub(crate) fn zip_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Result<Option<String>> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(ExtractError::Unreadable(error.to_string())),
    };
    // A document's XML is rarely over a few MB; a zip bomb's would be.
    if entry.size() > 256 << 20 {
        return Err(ExtractError::Unreadable(format!("{name} is too big")));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut bytes)
        .map_err(|error| ExtractError::Unreadable(error.to_string()))?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

pub(crate) fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let file =
        std::fs::File::open(path).map_err(|error| ExtractError::Unreadable(error.to_string()))?;
    zip::ZipArchive::new(file).map_err(|_| ExtractError::Unreadable("not a zip file".into()))
}

/// The body, then footnotes and endnotes, then headers and footers.
fn docx(path: &Path) -> Result<String> {
    let mut archive = open_zip(path)?;
    let body = zip_entry(&mut archive, "word/document.xml")?
        .ok_or_else(|| ExtractError::Unreadable("no word/document.xml inside".into()))?;
    let mut parts = vec![body];
    for name in ["word/footnotes.xml", "word/endnotes.xml"] {
        parts.extend(zip_entry(&mut archive, name)?);
    }
    let mut extra: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(|name| name.into_owned()))
        .filter(|name| name.starts_with("word/header") || name.starts_with("word/footer"))
        .collect();
    extra.sort();
    for name in extra {
        parts.extend(zip_entry(&mut archive, &name)?);
    }
    let text: Vec<String> = parts
        .iter()
        .map(|xml| plain_text_from_html(&as_html(xml, &["w:p"], &["w:br", "w:cr"], &["w:tab"])))
        .filter(|text| !text.is_empty())
        .collect();
    Ok(text.join("\n"))
}

fn odt(path: &Path) -> Result<String> {
    let mut archive = open_zip(path)?;
    let xml = zip_entry(&mut archive, "content.xml")?
        .ok_or_else(|| ExtractError::Unreadable("no content.xml inside".into()))?;
    Ok(plain_text_from_html(&as_html(
        &xml,
        &["text:p", "text:h"],
        &["text:line-break"],
        &["text:tab", "text:s"],
    )))
}

const RTF_SKIPPED_GROUPS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "header",
    "footer",
    "headerl",
    "headerr",
    "footerl",
    "footerr",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "themedata",
    "colorschememapping",
    "datastore",
    "xmlnstbl",
    "latentstyles",
    "pgdsctbl",
    "filetbl",
    "revtbl",
    "mmathPr",
    "fldinst",
    "object",
];

/// Rich Text Format, read for its words: control words dropped (paragraphs and lines kept as line breaks), escapes
/// decoded, destination groups (fonts, colors, styles, info, pictures) skipped. Enough for search.
pub fn rtf(rtf: &str) -> String {
    let chars: Vec<char> = rtf.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut skip_depth = 0; // inside a skipped group: its nesting depth
    let mut unicode_skip = 1; // \ucN: characters after \uN to skip
    let mut pending_skip = 0;
    let mut code_page_bytes: Vec<u8> = Vec::new(); // \'hh bytes waiting to be decoded together (multi-byte code pages)
    let flush = |bytes: &mut Vec<u8>, out: &mut String| {
        if !bytes.is_empty() {
            out.push_str(
                &encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(bytes)
                    .0,
            );
            bytes.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if skip_depth > 0 {
            match c {
                '{' => skip_depth += 1,
                '}' => skip_depth -= 1,
                '\\' => i += 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        if !(c == '\\' && chars.get(i + 1) == Some(&'\'')) {
            flush(&mut code_page_bytes, &mut out);
        }
        match c {
            '{' => {
                let mut j = i + 1;
                if chars.get(j) == Some(&'\\') {
                    j += 1;
                    if chars.get(j) == Some(&'*') {
                        // {\* …}: a destination readers may ignore
                        skip_depth = 1;
                        i = j + 1;
                        continue;
                    }
                    let mut word = String::new();
                    while j < chars.len() && chars[j].is_ascii_alphabetic() && word.len() < 32 {
                        word.push(chars[j]);
                        j += 1;
                    }
                    if RTF_SKIPPED_GROUPS.contains(&word.as_str()) {
                        skip_depth = 1;
                        i = j;
                        continue;
                    }
                }
                i += 1;
            }
            '}' => i += 1,
            '\\' => {
                i += 1;
                let Some(&next) = chars.get(i) else { break };
                if next == '\'' {
                    // \'hh: a byte in the document's code page (Windows-1252 here)
                    let hex: String = chars
                        .get(i + 1..i + 3)
                        .map(|h| h.iter().collect())
                        .unwrap_or_default();
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        if pending_skip > 0 {
                            pending_skip -= 1;
                        } else {
                            code_page_bytes.push(byte);
                        }
                        i += 3;
                    } else {
                        i += 1;
                    }
                } else if next.is_ascii_alphabetic() {
                    let mut word = String::new();
                    let mut number = String::new();
                    let mut j = i;
                    while j < chars.len() && chars[j].is_ascii_alphabetic() {
                        word.push(chars[j]);
                        j += 1;
                    }
                    if chars.get(j) == Some(&'-') {
                        number.push('-');
                        j += 1;
                    }
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        number.push(chars[j]);
                        j += 1;
                    }
                    if chars.get(j) == Some(&' ') {
                        j += 1; // the space ending a control word is part of it
                    }
                    match word.as_str() {
                        "par" | "line" | "sect" | "page" | "row" => out.push('\n'),
                        "tab" | "cell" => out.push('\t'),
                        "emdash" => out.push('—'),
                        "endash" => out.push('–'),
                        "lquote" => out.push('‘'),
                        "rquote" => out.push('’'),
                        "ldblquote" => out.push('“'),
                        "rdblquote" => out.push('”'),
                        "bullet" => out.push('•'),
                        "uc" => unicode_skip = number.parse().unwrap_or(1),
                        "u" => {
                            if let Ok(value) = number.parse::<i32>() {
                                let value = if value < 0 { value + 65536 } else { value } as u32;
                                if let Some(c) = char::from_u32(value) {
                                    out.push(c);
                                }
                            }
                            pending_skip = unicode_skip;
                        }
                        _ => {}
                    }
                    i = j;
                } else {
                    match next {
                        '\\' | '{' | '}' => out.push(next),
                        '~' => out.push(' '),
                        '\n' | '\r' => out.push('\n'),
                        _ => {} // \-, \_, \:, \| and other symbols
                    }
                    i += 1;
                }
            }
            '\r' | '\n' => i += 1,
            _ => {
                if pending_skip > 0 {
                    pending_skip -= 1;
                } else {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    flush(&mut code_page_bytes, &mut out);
    out
}

/// Word 97–2003: the text is in the WordDocument stream, in pieces the piece table (in the Clx, in the 0Table or
/// 1Table stream) lists; each piece is UTF-16 or "compressed" Windows-1252. Fields keep their result, not their code.
fn legacy_word(path: &Path) -> Result<String> {
    let unreadable = |why: &str| ExtractError::Unreadable(why.to_string());
    let mut file = cfb::open(path).map_err(|_| unreadable("not a Word 97–2003 document"))?;
    let mut word = Vec::new();
    file.open_stream("/WordDocument")
        .map_err(|_| unreadable("no WordDocument stream"))?
        .read_to_end(&mut word)
        .map_err(|error| unreadable(&error.to_string()))?;
    let u16_at = |b: &[u8], at: usize| b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32_at = |b: &[u8], at: usize| {
        b.get(at..at + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    if u16_at(&word, 0) != Some(0xA5EC) {
        return Err(unreadable("not a Word 97–2003 document"));
    }
    let flags = u16_at(&word, 0x0A).ok_or_else(|| unreadable("short header"))?;
    if flags & 0x0100 != 0 {
        return Err(ExtractError::Locked);
    }
    let table_name = if flags & 0x0200 != 0 {
        "/1Table"
    } else {
        "/0Table"
    };
    // FibBase (32 bytes), csw + fibRgW, cslw + fibRgLw, cbRgFcLcb, then fibRgFcLcb: fcClx/lcbClx are its 34th pair.
    let csw = u16_at(&word, 32).ok_or_else(|| unreadable("short header"))? as usize;
    let lw_at = 34 + csw * 2;
    let cslw = u16_at(&word, lw_at).ok_or_else(|| unreadable("short header"))? as usize;
    let fc_lcb = lw_at + 2 + cslw * 4 + 2;
    let fc_clx = u32_at(&word, fc_lcb + 33 * 8).ok_or_else(|| unreadable("short header"))? as usize;
    let lcb_clx =
        u32_at(&word, fc_lcb + 33 * 8 + 4).ok_or_else(|| unreadable("short header"))? as usize;
    let mut table = Vec::new();
    file.open_stream(table_name)
        .map_err(|_| unreadable("no table stream"))?
        .read_to_end(&mut table)
        .map_err(|error| unreadable(&error.to_string()))?;
    let clx = table
        .get(fc_clx..fc_clx + lcb_clx)
        .ok_or_else(|| unreadable("bad piece table"))?;
    // Skip the Prc entries (0x01, size, grpprl); the Pcdt (0x02) holds the piece table.
    let mut at = 0;
    while clx.get(at) == Some(&0x01) {
        at += 3 + u16_at(clx, at + 1).ok_or_else(|| unreadable("bad piece table"))? as usize;
    }
    if clx.get(at) != Some(&0x02) {
        return Err(unreadable("bad piece table"));
    }
    let lcb = u32_at(clx, at + 1).ok_or_else(|| unreadable("bad piece table"))? as usize;
    let plc = clx
        .get(at + 5..at + 5 + lcb)
        .ok_or_else(|| unreadable("bad piece table"))?;
    let pieces = (lcb.saturating_sub(4)) / 12;
    let mut text = String::new();
    for n in 0..pieces {
        let (Some(start), Some(end)) = (u32_at(plc, n * 4), u32_at(plc, n * 4 + 4)) else {
            break;
        };
        let count = end.saturating_sub(start) as usize;
        let Some(fc) = u32_at(plc, (pieces + 1) * 4 + n * 8 + 2) else {
            break;
        };
        if fc & 0x4000_0000 != 0 {
            let offset = ((fc & 0x3FFF_FFFF) / 2) as usize;
            if let Some(bytes) = word.get(offset..offset + count) {
                text.push_str(
                    &encoding_rs::WINDOWS_1252
                        .decode_without_bom_handling(bytes)
                        .0,
                );
            }
        } else {
            let offset = fc as usize;
            if let Some(bytes) = word.get(offset..offset + count * 2) {
                text.push_str(&encoding_rs::UTF_16LE.decode_without_bom_handling(bytes).0);
            }
        }
    }
    Ok(clean_word_text(&text))
}

/// Word's control characters: paragraph marks and breaks become line breaks, cell marks tabs; a field's code (between
/// its begin and separator marks) is dropped and its result kept; objects and the rest go.
fn clean_word_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // For each open field: whether we're still in its code (before the separator).
    let mut fields: Vec<bool> = Vec::new();
    for c in text.chars() {
        match c {
            '\u{13}' => fields.push(true),
            '\u{14}' => {
                if let Some(code) = fields.last_mut() {
                    *code = false;
                }
            }
            '\u{15}' => {
                fields.pop();
            }
            _ if fields.iter().any(|&code| code) => {}
            '\r' | '\u{0B}' | '\u{0C}' | '\u{0E}' => out.push('\n'),
            '\u{07}' | '\t' => out.push('\t'),
            '\u{1E}' => out.push('-'), // non-breaking hyphen
            '\u{A0}' => out.push(' '),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_text() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><h1>Lease</h1><p>Pets &amp; animals: \
                    one cat&nbsp;allowed.</p><script>alert(1)</script><!-- note --><p>Rent &#8364;1,450 &#x2014; due</p></body></html>";
        assert_eq!(
            plain_text_from_html(html),
            "Lease\nPets & animals: one cat allowed.\nRent €1,450 — due"
        );
    }

    #[test]
    fn rtf_becomes_text() {
        let source = concat!(
            r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\*\generator Word;}\f0 Caf\'e9 \b menu\b0\par Line two\tab end",
            r"\u",
            "8364?",
            r"\par}"
        );
        assert_eq!(rtf(source), "Café menu\nLine two\tend€\n");
    }

    #[test]
    fn word_fields_keep_their_result() {
        let raw = "See \u{13} HYPERLINK \"http://x\" \u{14}the site\u{15} now\rNext\u{07}cell";
        assert_eq!(clean_word_text(raw), "See the site now\nNext\tcell");
    }

    #[test]
    fn docx_and_odt_from_their_xml() {
        let dir = std::env::temp_dir().join(format!("trovara-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write_zip = |name: &str, entries: &[(&str, &str)]| {
            let path = dir.join(name);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            for (entry, body) in entries {
                zip.start_file(*entry, zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut zip, body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
            path
        };
        let docx_path = write_zip(
            "a.docx",
            &[("word/document.xml", "<w:document><w:body><w:p><w:r><w:t>Pets are</w:t></w:r><w:r><w:tab/><w:t>allowed</w:t></w:r></w:p><w:p><w:r><w:t>Rent &amp; deposit</w:t></w:r></w:p></w:body></w:document>")],
        );
        assert_eq!(
            text_of(&docx_path).unwrap(),
            "Pets are allowed\nRent & deposit"
        );
        let odt_path = write_zip(
            "b.odt",
            &[("content.xml", "<office:text><text:h>Title</text:h><text:p>One<text:s/>two<text:line-break/>three</text:p></office:text>")],
        );
        assert_eq!(text_of(&odt_path).unwrap(), "Title\nOne two\nthree");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
