//! Office files, e-books and saved emails as text (PRD F-RD-10/11/12, TRD §6.5c): Excel workbooks (each sheet's
//! cells, a tab between them, a line per row), PowerPoint decks (each slide's text and notes, "Slide N" first), EPUB
//! books (the chapters in reading order), .eml messages (headers, then the text part, any MIME encoding) and Outlook
//! .msg files (subject, people and body from the compound file's property streams).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::docs::{as_html, open_zip, plain_text_from_html, zip_entry};
use crate::{ExtractError, Result};

/// Cells read from one workbook at most: a sheet of data dumped to Excel can hold millions.
const MAX_CELLS: usize = 300_000;

static SHEET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<sheet\b[^>]*\bname="([^"]*)"[^>]*\br:id="([^"]*)""#).unwrap());
static RELATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<Relationship\b[^>]*\bId="([^"]*)"[^>]*\bTarget="([^"]*)"|<Relationship\b[^>]*\bTarget="([^"]*)"[^>]*\bId="([^"]*)""#).unwrap()
});
static SHARED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<si\b[^>]*>(.*?)</si>").unwrap());
static RUN_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<t\b[^>]*>(.*?)</t>").unwrap());
static ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<row\b[^>]*>(.*?)</row>").unwrap());
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<c\b([^>]*?)(?:/>|>(.*?)</c>)"#).unwrap());
static CELL_TYPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\bt="([^"]*)""#).unwrap());
static VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<v\b[^>]*>(.*?)</v>").unwrap());
static SLIDE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<p:sldId\b[^>]*\br:id="([^"]*)""#).unwrap());

/// XML's five entities and numeric references, as text (spaces kept: a cell's runs are joined as written).
fn unescape(text: &str) -> String {
    static NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").unwrap());
    let text = NUMERIC.replace_all(text, |caps: &regex::Captures| {
        let code = &caps[1];
        let value = match code.strip_prefix('x') {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => code.parse().ok(),
        };
        value.and_then(char::from_u32).map_or(String::new(), String::from)
    });
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// A package's relationships file: id → target (relative to the part it belongs to).
fn relations(xml: &str) -> HashMap<String, String> {
    RELATION
        .captures_iter(xml)
        .filter_map(|caps| {
            let (id, target) = match (caps.get(1), caps.get(2)) {
                (Some(id), Some(target)) => (id.as_str(), target.as_str()),
                _ => (caps.get(4)?.as_str(), caps.get(3)?.as_str()),
            };
            Some((id.to_string(), target.to_string()))
        })
        .collect()
}

/// `target` (from a relationship of a part in `folder`) as an archive path: "worksheets/sheet1.xml" in "xl" →
/// "xl/worksheets/sheet1.xml"; "/ppt/slides/slide1.xml" stays as it is (without the slash); "../x" goes up.
fn resolve(folder: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = folder.split('/').filter(|p| !p.is_empty()).collect();
    for piece in target.split('/') {
        match piece {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Every sheet's name, then its rows: the cells' values (shared strings looked up, numbers as written) with a tab
/// between them.
pub fn xlsx(path: &Path) -> Result<String> {
    let mut archive = open_zip(path)?;
    let workbook = zip_entry(&mut archive, "xl/workbook.xml")?
        .ok_or_else(|| ExtractError::Unreadable("no xl/workbook.xml inside".into()))?;
    let targets = zip_entry(&mut archive, "xl/_rels/workbook.xml.rels")?.map(|xml| relations(&xml)).unwrap_or_default();
    let shared: Vec<String> = zip_entry(&mut archive, "xl/sharedStrings.xml")?
        .map(|xml| {
            SHARED
                .captures_iter(&xml)
                .map(|item| RUN_TEXT.captures_iter(&item[1]).map(|run| unescape(&run[1])).collect::<String>())
                .collect()
        })
        .unwrap_or_default();
    let mut out = String::new();
    let mut cells = 0usize;
    for sheet in SHEET.captures_iter(&workbook) {
        let Some(target) = targets.get(&sheet[2]) else { continue };
        let Some(xml) = zip_entry(&mut archive, &resolve("xl", target))? else { continue };
        out.push_str(&unescape(&sheet[1]));
        out.push('\n');
        for row in ROW.captures_iter(&xml) {
            let mut values: Vec<String> = Vec::new();
            for cell in CELL.captures_iter(&row[1]) {
                let kind = CELL_TYPE.captures(&cell[1]).map(|t| t[1].to_string()).unwrap_or_default();
                let body = cell.get(2).map_or("", |b| b.as_str());
                let value = match kind.as_str() {
                    "s" => VALUE
                        .captures(body)
                        .and_then(|v| v[1].trim().parse::<usize>().ok())
                        .and_then(|i| shared.get(i).cloned())
                        .unwrap_or_default(),
                    "inlineStr" => RUN_TEXT.captures_iter(body).map(|run| unescape(&run[1])).collect(),
                    "b" => match VALUE.captures(body).map(|v| v[1].trim().to_string()).as_deref() {
                        Some("1") => "TRUE".into(),
                        Some("0") => "FALSE".into(),
                        _ => String::new(),
                    },
                    _ => VALUE.captures(body).map(|v| unescape(&v[1])).unwrap_or_default(),
                };
                values.push(value);
                cells += 1;
            }
            while values.last().is_some_and(|v| v.is_empty()) {
                values.pop();
            }
            if !values.is_empty() {
                out.push_str(&values.join("\t"));
                out.push('\n');
            }
            if cells >= MAX_CELLS {
                break;
            }
        }
        out.push('\n');
        if cells >= MAX_CELLS {
            break;
        }
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        return Err(ExtractError::Empty("no cells".into()));
    }
    Ok(text)
}

/// Each slide in the deck's order: "Slide N", its text, then its speaker notes.
pub fn pptx(path: &Path) -> Result<String> {
    let mut archive = open_zip(path)?;
    let presentation = zip_entry(&mut archive, "ppt/presentation.xml")?
        .ok_or_else(|| ExtractError::Unreadable("no ppt/presentation.xml inside".into()))?;
    let targets =
        zip_entry(&mut archive, "ppt/_rels/presentation.xml.rels")?.map(|xml| relations(&xml)).unwrap_or_default();
    let mut slides: Vec<String> = SLIDE_ID
        .captures_iter(&presentation)
        .filter_map(|caps| targets.get(&caps[1]).map(|target| resolve("ppt", target)))
        .collect();
    if slides.is_empty() {
        // No order in the deck: the slide files, by number.
        let mut found: Vec<(u32, String)> = archive
            .file_names()
            .filter_map(|name| name.ok().map(|n| n.into_owned()))
            .filter_map(|name| {
                let number = name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()?;
                Some((number, name))
            })
            .collect();
        found.sort();
        slides = found.into_iter().map(|(_, name)| name).collect();
    }
    let slide_text =
        |xml: &str| plain_text_from_html(&as_html(xml, &["a:p"], &["a:br"], &[])).trim().to_string();
    let mut out = Vec::new();
    for (index, slide) in slides.iter().enumerate() {
        let Some(xml) = zip_entry(&mut archive, slide)? else { continue };
        let mut part = format!("Slide {}\n{}", index + 1, slide_text(&xml));
        // The notes, through the slide's own relationships.
        let (folder, file) = slide.rsplit_once('/').unwrap_or(("", slide.as_str()));
        if let Some(rels) = zip_entry(&mut archive, &format!("{folder}/_rels/{file}.rels"))? {
            for target in relations(&rels).values().filter(|t| t.contains("notesSlide")) {
                if let Some(notes) = zip_entry(&mut archive, &resolve(folder, target))? {
                    // A notes page repeats the slide's number in a placeholder; its text is what was written.
                    let notes = slide_text(&notes);
                    let notes = notes.lines().filter(|line| line.trim().parse::<u32>().is_err()).collect::<Vec<_>>().join("\n");
                    if !notes.trim().is_empty() {
                        part.push_str("\nNotes: ");
                        part.push_str(notes.trim());
                    }
                }
            }
        }
        out.push(part);
    }
    if out.iter().all(|slide| slide.lines().count() <= 1) {
        return Err(ExtractError::Empty("no text on the slides".into()));
    }
    Ok(out.join("\n\n"))
}

static ROOTFILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<rootfile\b[^>]*\bfull-path="([^"]*)""#).unwrap());
static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<item\b([^>]*)/?>"#).unwrap());
static ITEMREF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<itemref\b[^>]*\bidref="([^"]*)""#).unwrap());
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b([\w:-]+)="([^"]*)""#).unwrap());
static BOOK_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<dc:title\b[^>]*>(.*?)</dc:title>").unwrap());

/// An EPUB book: its title, then its chapters in reading order (the spine), as text.
pub fn epub(path: &Path) -> Result<String> {
    let mut archive = open_zip(path)?;
    let container = zip_entry(&mut archive, "META-INF/container.xml")?
        .ok_or_else(|| ExtractError::Unreadable("not an EPUB (no META-INF/container.xml)".into()))?;
    let package_path = ROOTFILE
        .captures(&container)
        .map(|c| c[1].to_string())
        .ok_or_else(|| ExtractError::Unreadable("no package in the EPUB".into()))?;
    let package = zip_entry(&mut archive, &package_path)?
        .ok_or_else(|| ExtractError::Unreadable(format!("no {package_path} inside")))?;
    let folder = package_path.rsplit_once('/').map_or("", |(f, _)| f);
    let mut manifest: HashMap<String, String> = HashMap::new();
    for item in ITEM.captures_iter(&package) {
        let attributes: HashMap<&str, &str> = ATTRIBUTE
            .captures_iter(&item[1])
            .map(|a| (a.get(1).unwrap().as_str(), a.get(2).unwrap().as_str()))
            .collect();
        if let (Some(id), Some(href)) = (attributes.get("id"), attributes.get("href")) {
            manifest.insert(id.to_string(), resolve(folder, &percent_decoded(href)));
        }
    }
    let mut out = Vec::new();
    if let Some(title) = BOOK_TITLE.captures(&package) {
        out.push(unescape(&title[1]));
    }
    for reference in ITEMREF.captures_iter(&package) {
        let Some(href) = manifest.get(&reference[1]) else { continue };
        let Some(chapter) = zip_entry(&mut archive, href)? else { continue };
        let text = plain_text_from_html(&chapter);
        if !text.trim().is_empty() {
            out.push(text);
        }
    }
    if out.len() <= 1 {
        return Err(ExtractError::Empty("no chapters".into()));
    }
    Ok(out.join("\n\n"))
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&text[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// MARK: Email

/// A saved email (.eml, RFC 5322 + MIME): subject, from, to and date, then the body's text part (or its HTML as
/// text), decoded from base64 or quoted-printable and its charset.
pub fn eml(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| ExtractError::Unreadable(e.to_string()))?;
    let (headers, body) = split_message(&bytes);
    let mut out = String::new();
    for name in ["Subject", "From", "To", "Date"] {
        if let Some(value) = header(&headers, name) {
            out.push_str(&format!("{name}: {}\n", decode_words(&value)));
        }
    }
    let text = body_text(&headers, body, 0).unwrap_or_default();
    out.push('\n');
    out.push_str(text.trim());
    let out = out.trim().to_string();
    if out.is_empty() {
        return Err(ExtractError::Empty("an empty message".into()));
    }
    Ok(out)
}

/// The header block (unfolded, one header per line) and the body.
fn split_message(bytes: &[u8]) -> (Vec<(String, String)>, &[u8]) {
    let end = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, p + 4))
        .or_else(|| bytes.windows(2).position(|w| w == b"\n\n").map(|p| (p, p + 2)))
        .unwrap_or((bytes.len(), bytes.len()));
    let block = String::from_utf8_lossy(&bytes[..end.0]).replace("\r\n", "\n");
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in block.split('\n') {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    (headers, &bytes[end.1.min(bytes.len())..])
}

fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

/// A header parameter: `boundary="abc"` in `multipart/mixed; boundary="abc"`.
fn parameter(value: &str, name: &str) -> Option<String> {
    value.split(';').skip(1).find_map(|part| {
        let (key, val) = part.split_once('=')?;
        key.trim().eq_ignore_ascii_case(name).then(|| val.trim().trim_matches('"').to_string())
    })
}

/// The text of a (possibly multipart) body: the first text/plain part, else the first text/html part as text.
fn body_text(headers: &[(String, String)], body: &[u8], depth: usize) -> Option<String> {
    let content_type = header(headers, "Content-Type").unwrap_or_else(|| "text/plain".into());
    let kind = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
    if kind.starts_with("multipart/") && depth < 8 {
        let boundary = parameter(&content_type, "boundary")?;
        let marker = format!("--{boundary}");
        let text = String::from_utf8_lossy(body).into_owned();
        let parts: Vec<&str> = text.split(&marker).skip(1).filter(|p| !p.starts_with("--")).collect();
        let mut html = None;
        for part in parts {
            let (part_headers, part_body) = split_message(part.trim_start_matches(['\r', '\n']).as_bytes());
            let part_type = header(&part_headers, "Content-Type").unwrap_or_else(|| "text/plain".into()).to_ascii_lowercase();
            if part_type.starts_with("multipart/") {
                if let Some(found) = body_text(&part_headers, part_body, depth + 1) {
                    return Some(found);
                }
            } else if part_type.starts_with("text/plain") {
                return Some(decoded_part(&part_headers, part_body));
            } else if part_type.starts_with("text/html") && html.is_none() {
                html = Some(plain_text_from_html(&decoded_part(&part_headers, part_body)));
            }
        }
        return html;
    }
    let text = decoded_part(headers, body);
    Some(if kind == "text/html" { plain_text_from_html(&text) } else { text })
}

/// A part's body, its transfer encoding undone and its charset read.
fn decoded_part(headers: &[(String, String)], body: &[u8]) -> String {
    let encoding = header(headers, "Content-Transfer-Encoding").unwrap_or_default().to_ascii_lowercase();
    let raw: Vec<u8> = match encoding.trim() {
        "base64" => base64(body),
        "quoted-printable" => quoted_printable(body, false),
        _ => body.to_vec(),
    };
    let charset = header(headers, "Content-Type").and_then(|t| parameter(&t, "charset")).unwrap_or_else(|| "utf-8".into());
    decode_charset(&raw, &charset)
}

fn decode_charset(bytes: &[u8], charset: &str) -> String {
    match encoding_rs::Encoding::for_label(charset.trim().as_bytes()) {
        Some(encoding) => encoding.decode(bytes).0.into_owned(),
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// RFC 2047 encoded words in a header: "=?utf-8?B?SGVsbG8=?=" → "Hello".
fn decode_words(value: &str) -> String {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"=\?([^?]+)\?([bBqQ])\?([^?]*)\?=").unwrap());
    let joined = Regex::new(r"\?=\s+=\?").unwrap().replace_all(value, "?==?").into_owned();
    WORD.replace_all(&joined, |caps: &regex::Captures| {
        let bytes = if caps[2].eq_ignore_ascii_case("b") {
            base64(caps[3].as_bytes())
        } else {
            quoted_printable(caps[3].as_bytes(), true)
        };
        decode_charset(&bytes, &caps[1])
    })
    .into_owned()
}

fn base64(text: &[u8]) -> Vec<u8> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &c in text {
        if c == b'=' {
            break;
        }
        let Some(v) = value(c) else { continue };
        buffer = (buffer << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    out
}

/// Quoted-printable; `header`: "_" is a space (RFC 2047's Q encoding).
fn quoted_printable(text: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            b'=' if i + 1 < text.len() && (text[i + 1] == b'\n' || text[i + 1] == b'\r') => {
                // A soft line break.
                i += if text[i + 1] == b'\r' && text.get(i + 2) == Some(&b'\n') { 3 } else { 2 };
            }
            b'=' if i + 2 < text.len() => match u8::from_str_radix(&String::from_utf8_lossy(&text[i + 1..i + 3]), 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'=');
                    i += 1;
                }
            },
            b'_' if header => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

/// An Outlook message (.msg, a compound file): subject, sender, recipients and body, from the message's property
/// streams (Unicode "001F" ones, else "001E" in the system code page).
pub fn msg(path: &Path) -> Result<String> {
    let mut file = cfb::open(path).map_err(|e| ExtractError::Unreadable(format!("not an Outlook message: {e}")))?;
    let mut property = |id: &str| -> Option<String> {
        for (suffix, unicode) in [("001F", true), ("001E", false)] {
            let name = format!("/__substg1.0_{id}{suffix}");
            let Ok(mut stream) = file.open_stream(&name) else { continue };
            let mut bytes = Vec::new();
            if stream.read_to_end(&mut bytes).is_err() {
                continue;
            }
            let text = if unicode {
                let units: Vec<u16> = bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
                String::from_utf16_lossy(&units)
            } else {
                crate::text::decode(&bytes)
            };
            let text = text.trim_end_matches('\0').trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
        None
    };
    let mut out = String::new();
    for (label, id) in [("Subject", "0037"), ("From", "0C1A"), ("To", "0E04"), ("Cc", "0E03")] {
        if let Some(value) = property(id) {
            out.push_str(&format!("{label}: {value}\n"));
        }
    }
    let body = property("1000").or_else(|| property("1013").map(|html| plain_text_from_html(&html)));
    if let Some(body) = body {
        out.push('\n');
        out.push_str(body.trim());
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        return Err(ExtractError::Empty("an empty message".into()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip(path: &Path, files: &[(&str, &str)]) {
        let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let folder = std::env::temp_dir().join(format!("trovara-office-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        folder.join(name)
    }

    #[test]
    fn excel_cells_by_sheet() {
        let path = scratch("budget.xlsx");
        zip(&path, &[
            ("xl/workbook.xml", r#"<workbook><sheets><sheet name="Trip &amp; Food" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="rId1" Type="x" Target="worksheets/sheet1.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", r#"<sst><si><t>Hotel</t></si><si><r><t>Train </t></r><r><t>tickets</t></r></si></sst>"#),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>120.5</v></c></row><row r="2"><c r="A2" t="s"><v>1</v></c><c r="B2" t="inlineStr"><is><t>paid</t></is></c><c r="C2" t="b"><v>1</v></c></row></sheetData></worksheet>"#),
        ]);
        assert_eq!(xlsx(&path).unwrap(), "Trip & Food\nHotel\t120.5\nTrain tickets\tpaid\tTRUE");
    }

    #[test]
    fn slides_in_order_with_notes() {
        let path = scratch("deck.pptx");
        zip(&path, &[
            ("ppt/presentation.xml", r#"<p:presentation><p:sldIdLst><p:sldId id="257" r:id="rId3"/><p:sldId id="256" r:id="rId2"/></p:sldIdLst></p:presentation>"#),
            ("ppt/_rels/presentation.xml.rels", r#"<Relationships><Relationship Id="rId2" Target="slides/slide1.xml"/><Relationship Id="rId3" Target="slides/slide2.xml"/></Relationships>"#),
            ("ppt/slides/slide1.xml", r#"<p:sld><a:p><a:r><a:t>Budget</a:t></a:r></a:p><a:p><a:r><a:t>Q3 numbers</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/slide2.xml", r#"<p:sld><a:p><a:r><a:t>Welcome</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/_rels/slide2.xml.rels", r#"<Relationships><Relationship Id="rId1" Target="../notesSlides/notesSlide1.xml"/></Relationships>"#),
            ("ppt/notesSlides/notesSlide1.xml", r#"<p:notes><a:p><a:r><a:t>Say hello first</a:t></a:r></a:p><a:p><a:r><a:t>1</a:t></a:r></a:p></p:notes>"#),
        ]);
        assert_eq!(pptx(&path).unwrap(), "Slide 1\nWelcome\nNotes: Say hello first\n\nSlide 2\nBudget\nQ3 numbers");
    }

    #[test]
    fn epub_chapters_in_reading_order() {
        let path = scratch("book.epub");
        zip(&path, &[
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#),
            ("OEBPS/content.opf", r#"<package><metadata><dc:title>The Hills</dc:title></metadata><manifest><item id="c2" href="ch%202.xhtml" media-type="application/xhtml+xml"/><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#),
            ("OEBPS/ch1.xhtml", "<html><body><h1>One</h1><p>The walk began.</p></body></html>"),
            ("OEBPS/ch 2.xhtml", "<html><body><h1>Two</h1><p>The lake.</p></body></html>"),
        ]);
        assert_eq!(epub(&path).unwrap(), "The Hills\n\nOne\nThe walk began.\n\nTwo\nThe lake.");
    }

    #[test]
    fn email_text_part_any_encoding() {
        let path = scratch("note.eml");
        std::fs::write(&path, concat!(
            "From: Asha <asha@example.com>\r\nTo: me@example.com\r\nSubject: =?utf-8?B?Q2Fmw6kgcGxhbnM=?=\r\n",
            "Content-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n",
            "--b1\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n",
            "Lunch at the caf=C3=A9 by the lake=\r\n, at noon.\r\n",
            "--b1\r\nContent-Type: text/html\r\n\r\n<p>ignored</p>\r\n--b1--\r\n",
        )).unwrap();
        let text = eml(&path).unwrap();
        assert!(text.starts_with("Subject: Café plans\nFrom: Asha <asha@example.com>\nTo: me@example.com"), "{text}");
        assert!(text.ends_with("Lunch at the café by the lake, at noon."), "{text}");
    }

    #[test]
    fn package_paths() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("ppt/slides", "../notesSlides/n1.xml"), "ppt/notesSlides/n1.xml");
        assert_eq!(resolve("xl", "/xl/worksheets/sheet2.xml"), "xl/worksheets/sheet2.xml");
    }
}
