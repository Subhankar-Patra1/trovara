//! Hybrid search (TRD §8): meaning (EmbeddingGemma's vectors) + words (FTS5).
//!
//! Meaning is the model's cosine similarity, put on one scale per query (`meaning`): every vector is scored anyway, so
//! the distribution comes for free, and only segments that stand out (`z_gate`) count as meaning matches. That also
//! gives an honest "nothing found". Words rescue what a vector can't hold exactly: codes, IDs and names.
//!
//! The fusion formula beat reciprocal-rank fusion on the evals (R@1 0.88 vs 0.73): don't change it without running
//! them.

use std::collections::{BTreeSet, HashMap, HashSet};

use rusqlite::OptionalExtension;
use serde::Serialize;

use crate::query::{is_lookup, kind_hints, patterns, terms};
use crate::search_text::{excerpt, snippet};
use crate::store::{IndexStore, Info, Result};
use crate::vectors::{Matrix, DIMENSION};
use crate::{code, paths, FileKind, IndexOptions, Modality, SegmentKind};

#[derive(Clone, Debug, Serialize)]
pub struct SearchHit {
    pub path: String,
    pub kind: FileKind,
    /// Fused rank score, for ordering only.
    pub score: f64,
    /// How far the best segment stands out for this query.
    pub z: Option<f64>,
    pub cosine: Option<f64>,
    /// 1-based rank among keyword matches.
    pub keyword_rank: Option<usize>,
    pub segment: SegmentKind,
    pub segment_id: i64,
    /// PDF page, or seconds into audio/video, or the first line of a stretch of code.
    pub loc: Option<f64>,
    pub loc_end: Option<f64>,
    pub excerpt: Option<String>,
    /// Code: the line to open at. None: the stretch's first line.
    pub focus: Option<f64>,
    /// The file's modification date (seconds since 1970).
    pub modified: f64,
    pub size: i64,
    pub info: Info,
    /// The forms keyword search found in the file, as written ("payments" for "payment"): what previews mark besides
    /// the query's words (PRD F-PRE-02).
    pub found: Vec<String>,
}

/// How much finding the query's words counts against meaning. Code's words count for less: they're everywhere in code.
pub const WORD_WEIGHT: f64 = 2.0;
pub const CODE_WORD_WEIGHT: f64 = 1.0;
/// The bonus for a file whose own name holds the query's words (at full coverage; it falls with its square). People
/// name files for what they are: "PAN_Card_Front_View.jpeg" is what "pan card" means, even when the picture's meaning
/// match is weak. With the words' own bonus it puts such a file above meaning matches up to z ≈ 6.
pub const NAME_WEIGHT: f64 = 4.0;
/// Segments that stand out by at least this many standard deviations count as meaning matches.
pub const Z_GATE: f64 = 2.0;

pub struct SearchOptions<'a> {
    pub kinds: Option<&'a BTreeSet<FileKind>>,
    pub limit: usize,
    pub z_gate: f64,
    pub word_weight: Option<f64>,
    /// The last word is still being typed: match it as a prefix.
    pub prefix_last: bool,
}

impl Default for SearchOptions<'_> {
    fn default() -> Self {
        SearchOptions {
            kinds: None,
            limit: 20,
            z_gate: Z_GATE,
            word_weight: None,
            prefix_last: false,
        }
    }
}

pub struct Searcher {
    pub store: IndexStore,
    matrix: Option<Matrix>,
    row_file: Vec<i64>,
    row_segment: Vec<i64>,
    row_modality: Vec<Modality>,
    /// False for the vectors the later pass adds (the rest of long files): scored, but they don't set the scale.
    row_sets_scale: Vec<bool>,
    file_kind: HashMap<i64, FileKind>,
    /// Segments with keyword text (for telling common words from rare ones).
    segment_count: i64,
    pub vector_source: Option<String>,
    /// This is the code index's searcher.
    pub is_code: bool,
}

impl Searcher {
    /// With `load_vectors: false` only keyword search is ready; vectors load on the first meaning search. The app uses
    /// that to stay small while idle.
    pub fn new(store: IndexStore, load_vectors: bool) -> Result<Searcher> {
        let is_code = store
            .file
            .file_name()
            .is_some_and(|name| name == paths::CODE_INDEX_FILE);
        let mut searcher = Searcher {
            store,
            matrix: None,
            row_file: Vec::new(),
            row_segment: Vec::new(),
            row_modality: Vec::new(),
            row_sets_scale: Vec::new(),
            file_kind: HashMap::new(),
            segment_count: 0,
            vector_source: None,
            is_code,
        };
        if load_vectors {
            searcher.reload()?;
        } else {
            searcher.reload_files()?;
        }
        Ok(searcher)
    }

    pub fn vectors_loaded(&self) -> bool {
        self.matrix.is_some()
    }

    pub fn vector_count(&self) -> usize {
        self.row_file.len()
    }

    /// Reads the files and all vectors again (after indexing added some).
    pub fn reload(&mut self) -> Result<()> {
        self.release_vectors();
        self.reload_files()?;
        let rows: i64 = self.store.db.query_row(
            "SELECT COUNT(*) FROM segments WHERE vec IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let mut matrix = Matrix::with_capacity(rows as usize);
        let steps = IndexOptions::default();
        let mut statement = self.store.db.prepare(
            "SELECT s.id, s.file, s.modality, s.vec, s.kind, s.loc, f.kind, CAST(json_extract(f.info, '$.pages') AS INTEGER)
             FROM segments s JOIN files f ON f.id = s.file WHERE s.vec IS NOT NULL ORDER BY s.id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let blob: Vec<u8> = row.get(3)?;
            if !matrix.push_blob(&blob) {
                continue;
            }
            self.row_segment.push(row.get(0)?);
            self.row_file.push(row.get(1)?);
            let modality: String = row.get(2)?;
            self.row_modality.push(match modality.as_str() {
                "text" => Modality::Text,
                "image" => Modality::Image,
                "audio" => Modality::Audio,
                _ => Modality::None,
            });
            // Past the first step of a long PDF (its pages) or document (its chunks).
            let segment_kind: String = row.get(4)?;
            let place = row.get::<_, Option<f64>>(5)?.unwrap_or(0.0) as u32;
            let file_kind: String = row.get(6)?;
            let pages = row.get::<_, Option<i64>>(7)?.unwrap_or(0);
            let later = if file_kind == "pdf" {
                segment_kind != "chunk"
                    && pages > steps.big_pdf_pages as i64
                    && place > steps.pdf_step
            } else {
                segment_kind == "chunk" && place > steps.doc_step
            };
            self.row_sets_scale.push(!later);
        }
        self.matrix = Some(matrix);
        Ok(())
    }

    /// Takes the vectors (and the files' kinds) another searcher of the same index loaded: the search service loads
    /// them on a thread of its own, so keyword search isn't kept waiting meanwhile.
    pub fn adopt_vectors(&mut self, other: Searcher) {
        self.matrix = other.matrix;
        self.row_file = other.row_file;
        self.row_segment = other.row_segment;
        self.row_modality = other.row_modality;
        self.row_sets_scale = other.row_sets_scale;
        self.file_kind = other.file_kind;
        self.segment_count = other.segment_count;
        self.vector_source = other.vector_source;
    }

    /// Frees the vectors' memory; keyword search keeps working.
    pub fn release_vectors(&mut self) {
        self.matrix = None;
        self.row_file = Vec::new();
        self.row_segment = Vec::new();
        self.row_modality = Vec::new();
        self.row_sets_scale = Vec::new();
    }

    fn reload_files(&mut self) -> Result<()> {
        self.file_kind.clear();
        let mut statement = self.store.db.prepare("SELECT id, kind FROM files")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, kind) = row?;
            if let Some(kind) = FileKind::parse(&kind) {
                self.file_kind.insert(id, kind);
            }
        }
        self.vector_source = self.store.meta("vector_source")?;
        self.segment_count =
            self.store
                .db
                .query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))?;
        Ok(())
    }

    pub fn search(
        &mut self,
        text: &str,
        query_vector: Option<&[f32]>,
        options: &SearchOptions,
    ) -> Result<Vec<SearchHit>> {
        let word_weight = options.word_weight.unwrap_or(if self.is_code {
            CODE_WORD_WEIGHT
        } else {
            WORD_WEIGHT
        });
        if query_vector.is_some() && !self.vectors_loaded() {
            self.reload()?;
        }
        let allowed = |file: i64| match options.kinds {
            None => true,
            Some(kinds) => self
                .file_kind
                .get(&file)
                .is_some_and(|kind| kinds.contains(kind)),
        };

        // Meaning: score every vector on this query's scale, keep each file's best segment.
        let mut best: HashMap<i64, (f64, f64, usize)> = HashMap::new(); // file → (z, cosine, row)
        if let (Some(query), Some(matrix)) = (query_vector, &self.matrix) {
            if query.len() == DIMENSION && matrix.rows() > 0 {
                let scores = matrix.scores(query);
                let z = meaning(&scores, &self.row_modality, Some(&self.row_sets_scale));
                for row in 0..scores.len() {
                    let file = self.row_file[row];
                    if !allowed(file) {
                        continue;
                    }
                    if best.get(&file).is_none_or(|current| z[row] > current.0) {
                        best.insert(file, (z[row], scores[row] as f64, row));
                    }
                }
            }
        }

        // Words: FTS5 with Porter stemming over file names, folder names, OCR, page and document text.
        let terms = terms(text);
        // A name from code ("quantize_row_q8_0_ref", "refreshAccessToken") is matched as one phrase, as it's written.
        let identifier = if self.is_code { code::identifier(text) } else { None };
        let mut patterns = match &identifier {
            Some(words) => vec![code::phrase(words)],
            None => patterns(&terms, options.prefix_last),
        };
        // A word that's in no file as typed matches as the start of longer words, as the last word does while it's
        // being typed: "grok" finds "Grok4". A word that is in the index as typed stays whole ("cat" isn't
        // "category"); so does a word of two letters, the start of too many words to stand for one.
        for index in 0..terms.len() {
            if identifier.is_none()
                && terms[index].chars().count() >= 3
                && !patterns[index].ends_with('*')
                && !self.is_indexed(&patterns[index])?
            {
                patterns[index].push('*');
            }
        }
        let mut words: Vec<(i64, i64, Option<String>)> = Vec::new(); // (file, segment, snippet)
        let mut coverage: HashMap<i64, f64> = HashMap::new(); // share of the query's words found anywhere in the file
        let mut name_coverage: HashMap<i64, f64> = HashMap::new(); // share of them in the file's name
        if !patterns.is_empty() {
            let mut seen = HashSet::new();
            // Ranked by FTS5 alone; the snippets are made later for the hits shown only. FTS5's own snippet() re-runs
            // the whole match for every row: with a short prefix ("ca*", 47k segments) 100 snippets took 850 ms.
            let mut statement = self.store.db.prepare(
                "SELECT rowid FROM fts WHERE fts MATCH ? ORDER BY bm25(fts, 3.0, 1.0) LIMIT 1000",
            )?;
            let mut file_of = self.store.db.prepare_cached("SELECT file FROM segments WHERE id = ?")?;
            let mut rows = statement.query([self.ranked_patterns(&patterns)?.join(" OR ")])?;
            while let Some(row) = rows.next()? {
                if words.len() >= 100 {
                    break;
                }
                let segment: i64 = row.get(0)?;
                let Some(file) = file_of.query_row([segment], |row| row.get::<_, i64>(0)).optional()? else { continue };
                if !allowed(file) || !seen.insert(file) {
                    continue;
                }
                words.push((file, segment, None));
            }
            if !words.is_empty() {
                // Which of the candidates' segments (≤ 100 files) hold each word. Driven from those segments, not from
                // every match of the word: a word in most of the index cost 54 ms a word that way at 100k segments.
                let files = words
                    .iter()
                    .map(|(file, _, _)| file.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let mut segment_file: HashMap<i64, i64> = HashMap::new();
                let mut names: HashSet<i64> = HashSet::new(); // the candidates' name segments
                {
                    let mut statement = self.store.db.prepare(&format!(
                        "SELECT id, file, kind = 'name' FROM segments WHERE file IN ({files})"
                    ))?;
                    for row in statement.query_map([], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, bool>(2)?))
                    })? {
                        let (segment, file, name) = row?;
                        segment_file.insert(segment, file);
                        if name {
                            names.insert(segment);
                        }
                    }
                }
                let (low, high) = (
                    segment_file.keys().copied().min().unwrap_or(0),
                    segment_file.keys().copied().max().unwrap_or(0),
                );
                let ids = if segment_file.len() > 2_000 { String::new() } else { segment_file
                    .keys()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",") };
                // An exact word seeks each segment (fast). A prefix ("not*") can't: FTS5 builds its whole match list
                // for every lookup, so it's read once, in rowid order within the candidates' range.
                let seek = segment_file.len() <= 2_000;
                let mut exact = if seek {
                    Some(self.store.db.prepare(&format!("SELECT rowid FROM fts WHERE fts MATCH ?1 AND rowid IN ({ids})"))?)
                } else {
                    None
                };
                let mut prefix = self.store.db.prepare(
                    "SELECT rowid FROM fts WHERE fts MATCH ?1 AND rowid BETWEEN ?2 AND ?3",
                )?;
                // …but only while the candidates have few segments: seeking each of 30k (long videos and PDFs) cost 600 ms
                // for one word, where reading the word's whole match list takes under a millisecond.
                for pattern in &patterns {
                    let found: Vec<i64> = if pattern.ends_with('*') || exact.is_none() {
                        prefix
                            .query_map(rusqlite::params![pattern, low, high], |row| row.get(0))?
                            .collect::<Result<_>>()?
                    } else if let Some(exact) = exact.as_mut() {
                        exact.query_map([pattern], |row| row.get(0))?.collect::<Result<_>>()?
                    } else {
                        Vec::new()
                    };
                    let holding: HashSet<i64> = found
                        .iter()
                        .filter_map(|segment| segment_file.get(segment).copied())
                        .collect();
                    for file in holding {
                        *coverage.entry(file).or_default() += 1.0 / patterns.len() as f64;
                    }
                    let in_name: HashSet<i64> = found
                        .iter()
                        .filter(|segment| names.contains(segment))
                        .filter_map(|segment| segment_file.get(segment).copied())
                        .collect();
                    for file in in_name {
                        *name_coverage.entry(file).or_default() += 1.0 / patterns.len() as f64;
                    }
                }
            }
        }

        // A file qualifies by standing out in meaning, or by containing at least half of the query's words.
        // Score: meaning z + a bonus that grows with word coverage, + 1 for the kind the query asks for. One common
        // word can't beat a strong meaning match; all the words of an exact lookup usually do.
        let lookup = is_lookup(&terms) || identifier.is_some();
        let (meaning_weight, words_weight) = if lookup {
            (0.5, word_weight * 2.0)
        } else {
            (1.0, word_weight)
        };
        let hinted = kind_hints(text);
        let mut candidates: HashSet<i64> = best
            .iter()
            .filter(|(_, (z, _, _))| *z >= options.z_gate)
            .map(|(file, _)| *file)
            .collect();
        candidates.extend(
            coverage
                .iter()
                .filter(|(_, share)| **share >= 0.5)
                .map(|(file, _)| *file),
        );
        let mut scored: Vec<(i64, f64)> = candidates
            .into_iter()
            .map(|file| {
                let z = best.get(&file).map_or(0.0, |b| b.0).max(0.0);
                let mut score = meaning_weight * z
                    + words_weight * coverage.get(&file).copied().unwrap_or(0.0).powf(1.5);
                // Most of the words in its name: the file is named for what was asked (not for code, whose file
                // names are its identifiers and are matched as such).
                let named = name_coverage.get(&file).copied().unwrap_or(0.0);
                if !self.is_code && named >= 0.5 {
                    score += NAME_WEIGHT * named.powi(2);
                }
                if self
                    .file_kind
                    .get(&file)
                    .is_some_and(|kind| hinted.contains(kind))
                {
                    score += 1.0;
                }
                (file, score)
            })
            .collect();
        let starts: Vec<String> = patterns
            .iter()
            .filter(|p| p.ends_with('*'))
            .map(|p| p.trim_end_matches('*').trim_matches('"').to_lowercase())
            .filter(|p| !p.is_empty())
            .collect();
        let mut word_rank: HashMap<i64, (usize, i64, Option<String>)> = HashMap::new();
        for (rank, (file, segment, quoted)) in words.into_iter().enumerate() {
            word_rank.entry(file).or_insert((rank + 1, segment, quoted));
        }
        // Equal scores (every file with all the words, before the meaning results are in) go by keyword rank: BM25,
        // where a match in the file's name weighs three times one in its text. Not by when the file was indexed: a
        // search for "voter card" listed older PDFs that mention a card above "Front_View of Voter_Card.png".
        let keyword_order = |file: &i64| word_rank.get(file).map_or(usize::MAX, |(rank, _, _)| *rank);
        scored.sort_by(|a, b| {
            b.1.total_cmp(&a.1).then(keyword_order(&a.0).cmp(&keyword_order(&b.0))).then(a.0.cmp(&b.0))
        });
        let mut hits = Vec::new();
        for (file, score) in scored.into_iter().take(options.limit) {
            let semantic = best.get(&file).copied();
            // The words in context, from the passage's own text (as written: with its hamza, harakat, joiners).
            let keyword = word_rank.get(&file).map(|(rank, segment, _)| (*rank, *segment, self.quote(*segment, &terms, &starts)));
            let keyword = keyword.as_ref();
            // Point at the best meaning segment if it stands out; else at the segment the words matched.
            let use_meaning =
                semantic.is_some_and(|(z, _, _)| z >= options.z_gate || keyword.is_none());
            let segment_id = if use_meaning {
                semantic.map(|(_, _, row)| self.row_segment[row])
            } else {
                keyword.map(|(_, segment, _)| *segment)
            };
            let Some(mut segment_id) = segment_id else {
                continue;
            };
            let mut focus = None;
            if self.is_code {
                // Where a name is defined is looked for in the first 10 results only: a name used all over a big
                // repo (ggml_tensor) is in hundreds of stretches of every file that has it (the reference measured
                // 560 → 100 ms a search on llama.cpp that way).
                let words_here = keyword.is_some_and(|(_, segment, _)| *segment == segment_id) || identifier.is_some();
                let name = if hits.len() < 10 { identifier.as_deref() } else { None };
                (segment_id, focus) = self.code_place(file, segment_id, name, text, &terms, words_here)?;
            }
            let mut hit = self.hit(file, score, segment_id, semantic, keyword)?;
            hit.focus = focus;
            hits.push(hit);
        }
        Ok(hits)
    }

    /// Code: the stretch to point at, and the line in it to open at. A name looked up points at the stretch that
    /// defines it, when one does (not the ones that call it). Otherwise the line is the first with the query's words,
    /// when they matched in this stretch; a match by meaning alone opens at the stretch's first line.
    fn code_place(
        &self,
        file: i64,
        segment: i64,
        identifier: Option<&[String]>,
        query: &str,
        terms: &[String],
        words_here: bool,
    ) -> Result<(i64, Option<f64>)> {
        let name = query.trim().replace("()", "");
        if let Some(words) = identifier {
            let mut best: Option<(i64, usize, u8)> = None;
            let mut statement = self.store.db.prepare(
                "SELECT s.id, s.loc, coalesce(fts.original, fts.body) FROM fts JOIN segments s ON s.id = fts.rowid
                 WHERE fts MATCH ?1 AND s.file = ?2 ORDER BY s.loc LIMIT 300",
            )?;
            let mut rows = statement.query(rusqlite::params![code::phrase(words), file])?;
            while let Some(row) = rows.next()? {
                if best.is_some_and(|b| b.2 >= 2) {
                    break;
                }
                let (id, loc, text): (i64, Option<f64>, Option<String>) = (row.get(0)?, row.get(1)?, row.get(2)?);
                let Some(text) = text else { continue };
                if let Some((line, strength)) = code::definition(&name, &text, loc.unwrap_or(1.0) as usize) {
                    if strength > best.map_or(0, |b| b.2) {
                        best = Some((id, line, strength));
                    }
                }
            }
            if let Some((id, line, _)) = best {
                return Ok((id, Some(line as f64)));
            }
        }
        if !words_here {
            return Ok((segment, None));
        }
        let Some(text) = self.text_of_segment(segment)? else {
            return Ok((segment, None));
        };
        let first: Option<f64> =
            self.store.db.query_row("SELECT loc FROM segments WHERE id = ?", [segment], |row| row.get(0))?;
        let words = if identifier.is_none() { terms.to_vec() } else { vec![name.to_lowercase()] };
        Ok((segment, code::first_line(&words, &text, first.unwrap_or(1.0) as usize).map(|line| line as f64)))
    }

    fn hit(
        &self,
        file: i64,
        score: f64,
        segment_id: i64,
        semantic: Option<(f64, f64, usize)>,
        keyword: Option<&(usize, i64, Option<String>)>,
    ) -> Result<SearchHit> {
        let (path, kind, modified, size, info): (String, String, f64, i64, Option<String>) =
            self.store.db.query_row(
                "SELECT path, kind, mtime, size, info FROM files WHERE id = ?",
                [file],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        let (segment, loc, loc_end, stored_excerpt): (
            String,
            Option<f64>,
            Option<f64>,
            Option<String>,
        ) = self.store.db.query_row(
            "SELECT kind, loc, loc_end, excerpt FROM segments WHERE id = ?",
            [segment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        // Quote the matched words only when they come from the segment we point at, so a PDF hit on page 5 never
        // shows page 2's words. A segment with no place and no text of its own (a screenshot's image vector) can
        // borrow them from its file's OCR text, which describes the same picture.
        let marked = |text: &Option<String>| {
            text.as_ref()
                .filter(|text| text.contains('«'))
                .map(|text| excerpt(text, 400))
        };
        let mut quoted = keyword
            .filter(|(_, segment, _)| *segment == segment_id)
            .and_then(|(_, _, text)| marked(text));
        if quoted.is_none() && stored_excerpt.is_none() && loc.is_none() {
            if let Some((_, matched, text)) = keyword {
                let matched_kind: Option<String> = self
                    .store
                    .db
                    .query_row("SELECT kind FROM segments WHERE id = ?", [matched], |row| {
                        row.get(0)
                    })
                    .optional()?;
                if matched_kind.as_deref() != Some("name") {
                    quoted = marked(text);
                }
            }
        }
        Ok(SearchHit {
            path,
            kind: FileKind::parse(&kind).unwrap_or(FileKind::Doc),
            score,
            z: semantic.map(|(z, _, _)| z),
            cosine: semantic.map(|(_, cosine, _)| cosine),
            keyword_rank: keyword.map(|(rank, _, _)| *rank),
            segment: segment_kind(&segment),
            segment_id,
            loc,
            loc_end,
            excerpt: quoted.or(stored_excerpt),
            focus: None,
            modified,
            size,
            info: info
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default(),
            found: keyword
                .and_then(|(_, _, text)| text.as_deref())
                .map(crate::search_text::marked_words)
                .unwrap_or_default(),
        })
    }

    /// The words of `text` that keyword search matches for the query's `terms`, as written: "months" and "Month" for
    /// "month", "paid" never (PRD F-PRE-02). Decided by SQLite's own FTS5, with the index's tokenizer, so a preview
    /// marks exactly what search matched, stemming and accents included.
    /// The snippet of `segment` around the query's words (marked « »), in the forms keyword search matched
    /// ("payments" for "payment"); None when the words aren't in its text (a match in the file's name).
    fn quote(&self, segment: i64, terms: &[String], starts: &[String]) -> Option<String> {
        let text: String = self
            .store
            .db
            .query_row("SELECT coalesce(original, body) FROM fts WHERE rowid = ?", [segment], |row| row.get(0))
            .optional()
            .ok()
            .flatten()?;
        // The words as typed usually do (a word's start: "payment" marks "payments"); only when they don't, the forms
        // FTS5 matched ("studies" for "study"), which cost a scratch index per passage.
        // A word matched by its start ("grok*" while it's typed, or a word in no file) marks the whole words it starts
        // ("Grok4"), found by a scan of the text.
        let mut typed = terms.to_vec();
        if !starts.is_empty() {
            for word in text.split(|c: char| !crate::search_text::is_word_char(c)) {
                let folded = word.to_lowercase();
                if starts.iter().any(|start| folded.starts_with(start.as_str())) && !typed.iter().any(|t| t == word) {
                    typed.push(word.to_string());
                }
            }
        }
        snippet(&text, &typed, 12).or_else(|| {
            let mut words = self.matching_words(&text, terms).unwrap_or_default();
            words.extend(terms.iter().cloned());
            snippet(&text, &words, 12)
        })
    }

    pub fn matching_words(&self, text: &str, terms: &[String]) -> Result<Vec<String>> {
        matching_words_with(&self.store.db, "temp.preview_words", text, terms)
    }

    /// The text of a page, a document's passage or a screenshot's OCR, as written: the preview shows the part of a
    /// document that matched.
    pub fn text_of_segment(&self, id: i64) -> Result<Option<String>> {
        self.store
            .db
            .query_row(
                "SELECT coalesce(original, body) FROM fts WHERE rowid = ?",
                [id],
                |row| row.get(0),
            )
            .optional()
    }

    /// The patterns the ranked keyword query uses: all of them, except words in more than a fifth of the index (and
    /// at least 5,000 segments) when the query has rarer ones too. BM25 gives such words almost no weight, yet ranking
    /// their tens of thousands of matches cost 145 ms at 100k segments. They still count in coverage, and a query of
    /// common words only is ranked on all of them. (TRD §8.2.)
    fn ranked_patterns(&self, patterns: &[String]) -> Result<Vec<String>> {
        let cap = (self.segment_count / 5).max(5_000);
        if self.segment_count < cap || patterns.len() < 2 {
            return Ok(patterns.to_vec());
        }
        let mut statement = self
            .store
            .db
            .prepare("SELECT COUNT(*) FROM (SELECT rowid FROM fts WHERE fts MATCH ? LIMIT ?)")?;
        let mut rare = Vec::new();
        for pattern in patterns {
            let count: i64 =
                statement.query_row(rusqlite::params![pattern, cap + 1], |row| row.get(0))?;
            if count <= cap {
                rare.push(pattern.clone());
            }
        }
        Ok(if rare.is_empty() {
            patterns.to_vec()
        } else {
            rare
        })
    }

    /// The query with each word that's in no file, not even as the start of one, replaced by the closest word the
    /// index has: one letter off (two for words of 8 letters or more), the commoner on a tie. "beyound" → "beyond".
    /// While typing (`prefix_last`) the last word may also be one letter off the start of an indexed word ("beyou" →
    /// "beyon…"), so results don't vanish mid-word. None when nothing needs correcting (or for code, whose names are
    /// spelled as they are).
    pub fn spelled(&self, text: &str, prefix_last: bool) -> Result<Option<String>> {
        if self.is_code {
            return Ok(None);
        }
        let lower = text.to_lowercase();
        if lower.len() != text.len() {
            return Ok(None); // offsets in the lowercase text must be the original's
        }
        let words = terms(text);
        let mut fixed = text.to_string();
        let mut changed = false;
        for (index, word) in words.iter().enumerate() {
            let length = word.chars().count();
            let last = prefix_last && index + 1 == words.len();
            if length < 4 || !word.chars().all(char::is_alphabetic) {
                continue;
            }
            if self.is_indexed(&format!("\"{word}\""))? || self.is_indexed(&format!("\"{word}\"*"))? {
                continue;
            }
            let Some(better) = self.closest_word(word, length, last)? else { continue };
            // Replace the word where it is in the query, keeping everything else as typed.
            if let Some(at) = find_word(&fixed.to_lowercase(), word) {
                fixed.replace_range(at..at + word.len(), &better);
                changed = true;
            }
        }
        Ok(changed.then_some(fixed))
    }

    /// The indexed word closest to `word` (it starts with the same letter): see `spelled`. The index keeps stems
    /// ("security" as "secur"), so a word is compared whole and also as a stem plus its own ending ("secruity" →
    /// "secur" + "ity"); every candidate is checked against the index before it's offered.
    fn closest_word(&self, word: &str, length: usize, prefix: bool) -> Result<Option<String>> {
        self.store.db.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS temp.fts_words USING fts5vocab(main, 'fts', 'row');")?;
        let first: String = word.chars().take(1).collect();
        let next = first.chars().next().and_then(|c| char::from_u32(c as u32 + 1)).map(String::from).unwrap_or_default();
        let limit = if length >= 8 { 2 } else { 1 };
        let typed: Vec<char> = word.chars().collect();
        // (distance, −files, how it was found: 0 whole, 1 stem + ending, 2 a word's start), the word
        let mut found: Vec<((usize, i64, u8), String)> = Vec::new();
        let mut statement = self.store.db.prepare("SELECT term, doc FROM temp.fts_words WHERE term >= ?1 AND term < ?2")?;
        let mut rows = statement.query(rusqlite::params![first, next])?;
        while let Some(row) = rows.next()? {
            let term: String = row.get(0)?;
            let docs: i64 = row.get(1)?;
            let chars: Vec<char> = term.chars().collect();
            let n = chars.len();
            if (n < 3 || n > length + limit || !chars.iter().all(|c| c.is_alphabetic())) && !(prefix && n > length) {
                continue;
            }
            if n <= length + limit {
                if let Some(d) = edit_distance(&typed, &chars, limit) {
                    found.push(((d, -docs, 0), term.clone()));
                }
                // The stem, then the typed word's own ending.
                // The stem covers most of the word: an ending of at most 3 letters.
                for k in n.saturating_sub(1).max(length.saturating_sub(3)).max(4)..=(n + 1).min(length.saturating_sub(1)) {
                    if edit_distance(&typed[..k], &chars, limit).is_some() {
                        let whole: Vec<char> = chars.iter().chain(typed[k..].iter()).copied().collect();
                        if let Some(d) = edit_distance(&typed, &whole, limit) {
                            found.push(((d, -docs, 1), whole.into_iter().collect()));
                        }
                    }
                }
            }
            // While typing: one letter off the start of a longer word (that start a letter shorter, as long, or longer).
            if prefix && n > length {
                for k in [length - 1, length, length + 1] {
                    if k >= 4 && k < n {
                        if let Some(d) = edit_distance(&typed, &chars[..k], 1) {
                            found.push(((d, -docs, 2), chars[..k].iter().collect()));
                        }
                    }
                }
            }
        }
        found.retain(|(key, _)| key.0 > 0);
        found.sort();
        // The first few that keyword search really finds, each as it's written in the files ("certificate", not the
        // stem's "certifate"); the one written closest to what was typed wins ("invoice", not "invoke" for "invioce").
        type Offer = ((usize, (usize, i64, u8)), String);   // (written distance, the candidate's key), the word
        let mut offers: Vec<Offer> = Vec::new();
        for (key, candidate) in found.into_iter().take(40) {
            if offers.len() >= 6 {
                break;
            }
            if key.2 == 2 {
                if self.is_indexed(&format!("\"{candidate}\"*"))? {
                    offers.push(((key.0, key), candidate));
                }
                continue;
            }
            if self.is_indexed(&format!("\"{candidate}\""))? {
                let written = self.written_as(&candidate, word)?.unwrap_or(candidate);
                let distance = edit_distance(&typed, &written.chars().collect::<Vec<_>>(), 4).unwrap_or(9);
                offers.push(((distance, key), written));
            }
        }
        Ok(offers.into_iter().min().map(|(_, w)| w))
    }

    /// The word as it's written in the files that `candidate` matches, the one closest to what was typed.
    fn written_as(&self, candidate: &str, typed: &str) -> Result<Option<String>> {
        let typed: Vec<char> = typed.chars().collect();
        let start: String = typed.iter().take(3).collect();
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut statement = self.store.db.prepare("SELECT coalesce(original, body) FROM fts WHERE fts MATCH ?1 LIMIT 40")?;
        let mut rows = statement.query([format!("\"{candidate}\"")])?;
        while let Some(row) = rows.next()? {
            let text: Option<String> = row.get(0)?;
            for w in text.unwrap_or_default().to_lowercase().split(|c: char| !c.is_alphabetic()) {
                if w.starts_with(&start) && w.chars().count() >= 3 {
                    *counts.entry(w.to_string()).or_default() += 1;
                }
            }
        }
        let best = counts
            .into_iter()
            .filter_map(|(w, n)| edit_distance(&typed, &w.chars().collect::<Vec<_>>(), 3).map(|d| ((d, std::cmp::Reverse(n)), w)))
            .min();
        match best {
            Some((_, w)) if self.is_indexed(&format!("\"{w}\""))? => Ok(Some(w)),
            _ => Ok(None),
        }
    }

    /// Whether keyword search finds `pattern` in any file.
    fn is_indexed(&self, pattern: &str) -> Result<bool> {
        Ok(self
            .store
            .db
            .query_row(
                "SELECT 1 FROM fts WHERE fts MATCH ? LIMIT 1",
                [pattern],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

/// The byte offset of `word` in `text` as a whole word (both lowercase).
fn find_word(text: &str, word: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find(word).map(|i| i + from) {
        let before = text[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + word.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        if before && after {
            return Some(i);
        }
        from = i + word.len();
    }
    None
}

/// The edit distance between `a` and `b` (insertions, deletions, substitutions, and swapped neighbours), or None when
/// it's more than `limit`.
fn edit_distance(a: &[char], b: &[char], limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    (d[n][m] <= limit).then_some(d[n][m])
}

/// How many of `hits` (best first) are strong: scoring at least `ratio` × the top hit. The rest pass the gates but
/// trail far behind, and the app folds them under "weaker matches". 0.5 cut the list from 26.2 to 5.7 results per
/// query on the evals and never folded an expected answer.
pub fn strong_count(hits: &[SearchHit], ratio: f64) -> usize {
    match hits.first() {
        Some(top) if top.score > 0.0 => hits
            .iter()
            .take_while(|hit| hit.score >= top.score * ratio)
            .count(),
        _ => hits.len(),
    }
}

/// This query's meaning scores: each vector's cosine minus the average of its modality (text, image and audio vectors
/// sit at slightly different levels for the same query), in standard deviations of all of them together. Within a
/// modality that's the model's own order. The scale comes from the rows in `sets_scale` (all when None): the later pages
/// of long files are scored on it but don't move it.
pub fn meaning(scores: &[f32], modality: &[Modality], sets_scale: Option<&[bool]>) -> Vec<f64> {
    let counts = |row: usize| sets_scale.is_none_or(|sets| sets[row]);
    let means = modality_means(scores, modality, sets_scale);
    let centered: Vec<f64> = scores
        .iter()
        .zip(modality)
        .map(|(&s, m)| s as f64 - means[m])
        .collect();
    let (mut count, mut sum) = (0.0, 0.0);
    for (row, value) in centered.iter().enumerate() {
        if counts(row) {
            count += 1.0;
            sum += value;
        }
    }
    let mean = sum / f64::max(count, 1.0);
    let mut squares = 0.0;
    for (row, value) in centered.iter().enumerate() {
        if counts(row) {
            squares += (value - mean) * (value - mean);
        }
    }
    let deviation = (squares / f64::max(count, 1.0)).sqrt().max(0.02);
    centered
        .iter()
        .map(|value| (value - mean) / deviation)
        .collect()
}

/// The average score of each modality for this query. Small groups (< 20 vectors) take the average of all.
fn modality_means(
    scores: &[f32],
    modality: &[Modality],
    sets_scale: Option<&[bool]>,
) -> HashMap<Modality, f64> {
    let mut sum: HashMap<Modality, f64> = HashMap::new();
    let mut count: HashMap<Modality, usize> = HashMap::new();
    for row in 0..scores.len() {
        if sets_scale.is_none_or(|sets| sets[row]) {
            *sum.entry(modality[row]).or_default() += scores[row] as f64;
            *count.entry(modality[row]).or_default() += 1;
        }
    }
    let total: usize = count.values().sum();
    let all = sum.values().sum::<f64>() / f64::max(total as f64, 1.0);
    [
        Modality::None,
        Modality::Text,
        Modality::Image,
        Modality::Audio,
    ]
    .into_iter()
    .map(|m| match count.get(&m) {
        Some(&n) if n >= 20 => (m, sum[&m] / n as f64),
        _ => (m, all),
    })
    .collect()
}

fn segment_kind(text: &str) -> SegmentKind {
    match text {
        "image" => SegmentKind::Image,
        "ocr" => SegmentKind::Ocr,
        "page" => SegmentKind::Page,
        "page_image" => SegmentKind::PageImage,
        "chunk" => SegmentKind::Chunk,
        "audio" => SegmentKind::Audio,
        "frame" => SegmentKind::Frame,
        "lines" => SegmentKind::Lines,
        "cells" => SegmentKind::Cells,
        "subtitle" => SegmentKind::Subtitle,
        _ => SegmentKind::Name,
    }
}

/// `Searcher::matching_words` without an index (the preview renderer's thread): its own in-memory SQLite.
pub struct WordMatcher {
    db: rusqlite::Connection,
}

impl WordMatcher {
    pub fn new() -> Result<WordMatcher> {
        Ok(WordMatcher { db: rusqlite::Connection::open_in_memory()? })
    }

    pub fn matching(&self, text: &str, terms: &[String]) -> Result<Vec<String>> {
        matching_words_with(&self.db, "main.preview_words", text, terms)
    }
}

/// The words of `text` keyword search matches for `terms`, using a scratch FTS5 table `table` on `db`.
fn matching_words_with(db: &rusqlite::Connection, table: &str, text: &str, terms: &[String]) -> Result<Vec<String>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut distinct: Vec<&str> = text
        .split(|c: char| !crate::search_text::is_word_char(c))
        .filter(|word| word.chars().count() >= 2)
        .collect();
    distinct.sort_unstable();
    distinct.dedup();
    distinct.truncate(20_000);
    let name = table.rsplit('.').next().unwrap_or(table);
    db.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {table} USING fts5(w, tokenize = \"{}\"); DELETE FROM {table};",
        crate::store::TOKENIZER
    ))?;
    {
        let mut insert = db.prepare_cached(&format!("INSERT INTO {table}(w) VALUES (?)"))?;
        for word in &distinct {
            insert.execute([word])?;
        }
    }
    let mut found: Vec<String> = Vec::new();
    {
        let mut select = db.prepare_cached(&format!("SELECT w FROM {table} WHERE {name} MATCH ?"))?;
        for pattern in crate::query::patterns(terms, false) {
            let words: Vec<String> = select.query_map([pattern], |row| row.get(0))?.collect::<Result<_>>()?;
            found.extend(words);
        }
    }
    db.execute_batch(&format!("DELETE FROM {table};"))?;
    found.sort_unstable();
    found.dedup();
    Ok(found)
}

#[cfg(test)]
mod tests {
    #[test]
    fn matching_words_follow_keyword_search() {
        let matcher = super::WordMatcher::new().unwrap();
        let found = matcher
            .matching("Twelve months. The Month's rent was paid; monthly fees.", &["month".to_string()])
            .unwrap();
        // Porter stemming: months and Month match month; monthly stems differently (search doesn't match it either).
        assert_eq!(found, ["Month", "months"]);
        assert!(matcher.matching("anything", &[]).unwrap().is_empty());
    }

    use super::*;
    use crate::store::Candidate;

    #[test]
    fn spelling_distance() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(edit_distance(&c("beyound"), &c("beyond"), 1), Some(1));
        assert_eq!(edit_distance(&c("recieve"), &c("receive"), 1), Some(1)); // swapped neighbours
        assert_eq!(edit_distance(&c("beyound"), &c("behind"), 1), None);
        assert_eq!(find_word("see beyound it", "beyound"), Some(4));
        assert_eq!(find_word("beyounds beyound", "beyound"), Some(9));
    }

    fn add(
        store: &IndexStore,
        path: &str,
        text: &str,
        kind: FileKind,
        segment: SegmentKind,
        vector: Option<&[f32]>,
    ) -> i64 {
        let inode = path.len() as u64 + path.bytes().map(u64::from).sum::<u64>();
        let id = store
            .insert(&Candidate {
                path: path.into(),
                kind,
                size: 1,
                mtime: 1.0,
                inode,
                device: 1,
            })
            .unwrap();
        let modality = if segment == SegmentKind::Image {
            Modality::Image
        } else {
            Modality::Text
        };
        store
            .add_segment(
                id,
                segment,
                modality,
                Some(1.0),
                None,
                None,
                Some(text),
                vector,
            )
            .unwrap();
        id
    }

    fn paths(hits: &[SearchHit]) -> Vec<&str> {
        hits.iter().map(|hit| hit.path.as_str()).collect()
    }

    fn words_only(searcher: &mut Searcher, text: &str, prefix_last: bool) -> Vec<SearchHit> {
        searcher
            .search(
                text,
                None,
                &SearchOptions {
                    prefix_last,
                    ..SearchOptions::default()
                },
            )
            .unwrap()
    }

    #[test]
    fn bengali_and_arabic_words_stay_whole() {
        let store = IndexStore::in_memory().unwrap();
        add(
            &store,
            r"C:\notes\trip.txt",
            "সুন্দরবনে আমরা কুমিরও দেখেছি। قَالَ إِنَّ الْمَكْتَبَةَ تُغْلَقُ",
            FileKind::Doc,
            SegmentKind::Chunk,
            None,
        );
        let mut searcher = Searcher::new(store, false).unwrap();
        let bengali = words_only(&mut searcher, "কুমির", false);
        assert_eq!(paths(&bengali), [r"C:\notes\trip.txt"]);
        assert!(
            bengali[0].excerpt.as_deref().unwrap().contains("«কুমিরও»"),
            "{:?}",
            bengali[0].excerpt
        ); // the whole word
        assert_eq!(
            paths(&words_only(&mut searcher, "المكتبة", false)),
            [r"C:\notes\trip.txt"]
        );
        assert!(words_only(&mut searcher, "কলকাতা", false).is_empty());
    }

    #[test]
    fn a_word_in_no_file_is_found_at_the_start_of_longer_ones() {
        let store = IndexStore::in_memory().unwrap();
        add(
            &store,
            r"C:\shots\settings.png",
            "Model picker: Grok4 (default)",
            FileKind::Screenshot,
            SegmentKind::Ocr,
            None,
        );
        add(
            &store,
            r"C:\notes\wiki.txt",
            "Read the Grokipedia page about rivers.",
            FileKind::Doc,
            SegmentKind::Chunk,
            None,
        );
        add(
            &store,
            r"C:\notes\pets.txt",
            "The cat sleeps on the sofa.",
            FileKind::Doc,
            SegmentKind::Chunk,
            None,
        );
        add(
            &store,
            r"C:\notes\tags.txt",
            "Pick a category for each photo.",
            FileKind::Doc,
            SegmentKind::Chunk,
            None,
        );
        let mut searcher = Searcher::new(store, false).unwrap();
        // "grok" is in no file as a word. While it's typed and once typing stops, it finds the same files.
        let typing: BTreeSet<String> = words_only(&mut searcher, "grok", true)
            .into_iter()
            .map(|h| h.path)
            .collect();
        let stopped = words_only(&mut searcher, "grok", false);
        let stopped_paths: BTreeSet<String> = stopped.iter().map(|h| h.path.clone()).collect();
        assert_eq!(
            stopped_paths,
            [
                r"C:\notes\wiki.txt".to_string(),
                r"C:\shots\settings.png".to_string()
            ]
            .into()
        );
        assert_eq!(typing, stopped_paths);
        let shot = stopped
            .iter()
            .find(|h| h.path == r"C:\shots\settings.png")
            .unwrap();
        assert!(
            shot.excerpt.as_deref().unwrap().contains("«Grok4»"),
            "{:?}",
            shot.excerpt
        );
        // Another word with it: both count.
        assert_eq!(
            paths(&words_only(&mut searcher, "grok rivers", false)),
            [r"C:\notes\wiki.txt", r"C:\shots\settings.png"]
        );
        // A word that's in a file as typed stays whole: "cat" isn't "category". So do two letters.
        assert_eq!(
            paths(&words_only(&mut searcher, "cat", false)),
            [r"C:\notes\pets.txt"]
        );
        assert!(words_only(&mut searcher, "gr", false).is_empty());
    }

    fn unit(seed: u64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(17);
        let mut v: Vec<f32> = (0..DIMENSION)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 2001) as f32 / 1000.0 - 1.0
            })
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }

    #[test]
    fn meaning_finds_the_vector_that_stands_out() {
        let store = IndexStore::in_memory().unwrap();
        for n in 0..50 {
            add(
                &store,
                &format!(r"C:\pics\IMG_{n}.jpg"),
                "",
                FileKind::Image,
                SegmentKind::Image,
                Some(&unit(n)),
            );
        }
        let mut searcher = Searcher::new(store, true).unwrap();
        assert_eq!(searcher.vector_count(), 50);
        // A query close to picture 7 (its vector plus a little noise).
        let noise = unit(999);
        let mut query: Vec<f32> = unit(7)
            .iter()
            .zip(&noise)
            .map(|(a, b)| a + 0.3 * b)
            .collect();
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        query.iter_mut().for_each(|x| *x /= norm);
        let hits = searcher
            .search("a beach", Some(&query), &SearchOptions::default())
            .unwrap();
        assert_eq!(
            paths(&hits),
            [r"C:\pics\IMG_7.jpg"],
            "only the one that stands out"
        );
        let real = hits[0].z.unwrap();
        assert!(real > 5.0);
        // A query near nothing: among 50 random vectors the best stands out ~2σ by chance, far below a real match.
        let chance = searcher
            .search(
                "a beach",
                Some(&unit(12345)),
                &SearchOptions {
                    z_gate: f64::MIN,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        assert!(
            chance[0].z.unwrap() < 3.5 && real > 2.0 * chance[0].z.unwrap(),
            "{} vs {real}",
            chance[0].z.unwrap()
        );
        // Kinds filter.
        let docs: BTreeSet<FileKind> = [FileKind::Doc].into();
        assert!(searcher
            .search(
                "x",
                Some(&query),
                &SearchOptions {
                    kinds: Some(&docs),
                    ..SearchOptions::default()
                }
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn exact_codes_beat_meaning() {
        let store = IndexStore::in_memory().unwrap();
        add(
            &store,
            r"C:\docs\invoice.txt",
            "Invoice INV20931 for the March order, paid in full.",
            FileKind::Doc,
            SegmentKind::Chunk,
            Some(&unit(1)),
        );
        for n in 2..40 {
            add(
                &store,
                &format!(r"C:\docs\note{n}.txt"),
                "Something else entirely",
                FileKind::Doc,
                SegmentKind::Chunk,
                Some(&unit(n)),
            );
        }
        let mut searcher = Searcher::new(store, true).unwrap();
        // Meaning points at note 5; the code points at the invoice. A lookup weighs words double.
        let hits = searcher
            .search("INV20931", Some(&unit(5)), &SearchOptions::default())
            .unwrap();
        assert_eq!(hits[0].path, r"C:\docs\invoice.txt");
        assert!(hits[0].excerpt.as_deref().unwrap().contains("«INV20931»"));
    }

    /// NFR-03: meaning results for 100,000 vectors in ≤ 150 ms. Run with
    /// `cargo test --release -p trovara-core -- --ignored --nocapture search_speed`.
    #[test]
    #[ignore]
    fn search_speed_at_100k_vectors() {
        let store = IndexStore::in_memory().unwrap();
        store
            .transaction(|| {
                for n in 0..100_000u64 {
                    let kind = if n % 3 == 0 {
                        FileKind::Image
                    } else {
                        FileKind::Doc
                    };
                    let segment = if kind == FileKind::Image {
                        SegmentKind::Image
                    } else {
                        SegmentKind::Chunk
                    };
                    let text = if kind == FileKind::Doc {
                        format!("note number {n} about topic {}", n % 97)
                    } else {
                        String::new()
                    };
                    add(
                        &store,
                        &format!(r"C:\data\f{n}.txt"),
                        &text,
                        kind,
                        segment,
                        Some(&unit(n)),
                    );
                }
                Ok(())
            })
            .unwrap();
        let started = std::time::Instant::now();
        let mut searcher = Searcher::new(store, true).unwrap();
        println!(
            "loaded 100k vectors in {:.0} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let query = unit(42);
        let mut times = Vec::new();
        for _ in 0..10 {
            let started = std::time::Instant::now();
            let hits = searcher
                .search("topic 42 note", Some(&query), &SearchOptions::default())
                .unwrap();
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            assert!(!hits.is_empty());
        }
        times.sort_by(f64::total_cmp);
        let keyword_started = std::time::Instant::now();
        searcher
            .search(
                "topic 42 not",
                None,
                &SearchOptions {
                    prefix_last: true,
                    ..SearchOptions::default()
                },
            )
            .unwrap();
        let keyword_ms = keyword_started.elapsed().as_secs_f64() * 1000.0;
        println!("meaning + words search: median {:.1} ms, best {:.1} ms; keyword-only while typing {keyword_ms:.1} ms", times[5], times[0]);
        assert!(times[5] < 150.0, "NFR-03: {:.1} ms", times[5]);
        assert!(keyword_ms < 30.0, "NFR-02: {keyword_ms:.1} ms");
    }

    #[test]
    fn strong_count_folds_weak_hits() {
        let hit = |score: f64| SearchHit {
            path: String::new(),
            kind: FileKind::Doc,
            score,
            z: None,
            cosine: None,
            keyword_rank: None,
            segment: SegmentKind::Chunk,
            segment_id: 0,
            loc: None,
            loc_end: None,
            excerpt: None,
            focus: None,
            modified: 0.0,
            size: 0,
            info: Info::new(),
            found: Vec::new(),
        };
        assert_eq!(
            strong_count(&[hit(10.0), hit(6.0), hit(4.9), hit(4.0)], 0.5),
            2
        );
        assert_eq!(strong_count(&[], 0.5), 0);
    }
}
