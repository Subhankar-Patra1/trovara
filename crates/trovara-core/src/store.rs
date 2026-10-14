//! The on-disk index (TRD §4.2): one SQLite file with files, their segments (with float16 vectors), and an FTS5 table.
//!
//! Vectors live in the same database as everything else, so a crash can never leave them out of step with the rows
//! that describe them. At this scale (tens of thousands of vectors) brute-force search over an in-memory copy takes a
//! few milliseconds: no vector database needed.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use unicode_normalization::UnicodeNormalization;

use crate::search_text::normalized;
use crate::{vectors, FileKind, Modality, SegmentKind};

pub type Result<T> = std::result::Result<T, rusqlite::Error>;

/// Trovara's schema. Changes ship as one migration each.
pub const SCHEMA_VERSION: i64 = 1;

/// Keyword search's tokenizer. Marks (M*) count as letters, so Bengali vowel signs and Arabic harakat stay inside their
/// words: by default SQLite splits on them ("কলকাতায়" became "কলক", "ত", "য").
pub const TOKENIZER: &str = "porter unicode61 remove_diacritics 2 categories 'L* N* Co M*'";

/// A file as the crawl found it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: String,
    pub kind: FileKind,
    pub size: i64,
    pub mtime: f64,
    pub inode: u64,
    pub device: u64,
}

/// A file as the index knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub kind: FileKind,
    pub size: i64,
    pub mtime: f64,
    pub inode: u64,
    pub device: u64,
    pub state: String,
}

/// What indexing learned about a file: "width", "height", "taken", "pages", "duration", "read_to"…
pub type Info = BTreeMap<String, String>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Counts {
    pub by_kind_and_state: BTreeMap<String, BTreeMap<String, i64>>,
    pub vectors: i64,
    pub segments: i64,
    /// Long files still being read, and their pages and chunks left.
    pub unfinished: i64,
    pub left: i64,
}

pub struct IndexStore {
    pub db: Connection,
    pub file: PathBuf,
    /// When set, `pending`, `pending_count` and `counts` see only files under these folders: the helper sets the
    /// folders that are connected right now, so files on an unplugged drive neither fail nor count as waiting.
    scope: Option<Vec<String>>,
}

const FILE_COLUMNS: &str = "id, path, kind, size, mtime, inode, device, state";

impl IndexStore {
    /// Opens (or creates) `name` in `directory`: `index.sqlite`, or `code.sqlite` for the code index.
    pub fn open(directory: &Path, name: &str) -> Result<IndexStore> {
        std::fs::create_dir_all(directory)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let file = directory.join(name);
        let db = Connection::open(&file)?;
        Self::configure(&db)?;
        let store = IndexStore {
            db,
            file,
            scope: None,
        };
        store.migrate()?;
        Ok(store)
    }

    /// An index in memory, for tests.
    pub fn in_memory() -> Result<IndexStore> {
        let db = Connection::open_in_memory()?;
        Self::configure(&db)?;
        let store = IndexStore {
            db,
            file: PathBuf::from(":memory:"),
            scope: None,
        };
        store.migrate()?;
        Ok(store)
    }

    fn configure(db: &Connection) -> Result<()> {
        db.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
        )
    }

    /// Bytes on disk: the database and its write-ahead log.
    pub fn size_on_disk(&self) -> u64 {
        [
            self.file.clone(),
            PathBuf::from(format!("{}-wal", self.file.display())),
        ]
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum()
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self
            .db
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "this index was made by a newer Trovara (schema {version})"
            )));
        }
        if version == SCHEMA_VERSION {
            return Ok(());
        }
        // The app and its helper can open the index at the same moment: one migrates while the other waits.
        self.db
            .execute_batch("PRAGMA busy_timeout = 60000; BEGIN IMMEDIATE;")?;
        let result = (|| {
            let version: i64 = self
                .db
                .query_row("PRAGMA user_version", [], |row| row.get(0))?;
            if version == 0 {
                self.create()?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.db.execute_batch("COMMIT; PRAGMA busy_timeout = 5000;"),
            Err(error) => {
                let _ = self
                    .db
                    .execute_batch("ROLLBACK; PRAGMA busy_timeout = 5000;");
                Err(error)
            }
        }
    }

    fn create(&self) -> Result<()> {
        self.db.execute_batch(&format!(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE files(
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,      -- as the crawl reports it
                path_key TEXT NOT NULL,         -- folded (NFC, lowercase, backslashes): Windows paths ignore case
                kind TEXT NOT NULL,
                size INTEGER NOT NULL,
                mtime REAL NOT NULL,
                inode INTEGER NOT NULL,         -- NTFS file reference number
                device INTEGER NOT NULL,        -- volume serial number
                file_id128 BLOB,                -- ReFS: the full 128-bit file ID
                state TEXT NOT NULL DEFAULT 'pending',   -- pending | done | partial | skipped | failed
                detail TEXT,                    -- why skipped/failed, or how far a long file has been read
                info TEXT,                      -- JSON: width, height, taken, pages, duration; read_to/read_end
                indexed_at REAL
            );
            CREATE INDEX files_inode ON files(device, inode);
            CREATE INDEX files_state ON files(state, kind);
            CREATE INDEX files_path_key ON files(path_key);
            CREATE TABLE segments(
                id INTEGER PRIMARY KEY,
                file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                kind TEXT NOT NULL,       -- SegmentKind
                modality TEXT NOT NULL,   -- Modality of the vector ('none' when there is no vector)
                loc REAL,                 -- page number, start second, or first line/cell
                loc_end REAL,
                excerpt TEXT,
                vec BLOB                  -- 768 × float16 LE, L2-normalized
            );
            CREATE INDEX segments_file ON segments(file);
            CREATE VIRTUAL TABLE fts USING fts5(name, body, original UNINDEXED, tokenize = \"{TOKENIZER}\");
            PRAGMA user_version = {SCHEMA_VERSION};"
        ))
    }

    // MARK: Meta

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.db
            .query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
                row.get(0)
            })
            .optional()
    }

    pub fn set_meta(&self, key: &str, value: Option<&str>) -> Result<()> {
        match value {
            Some(value) => self.db.execute(
                "INSERT INTO meta(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            ),
            None => self.db.execute("DELETE FROM meta WHERE key = ?", [key]),
        }
        .map(|_| ())
    }

    // MARK: Scope

    pub fn set_scope(&mut self, scope: Option<Vec<String>>) {
        self.scope = scope;
    }

    /// The SQL and values that keep a query to `scope`.
    fn scope_sql(&self) -> (String, Vec<String>) {
        let Some(scope) = &self.scope else {
            return (String::new(), Vec::new());
        };
        let ranges: Vec<(String, String)> = scope
            .iter()
            .flat_map(|folder| path_ranges(folder))
            .collect();
        if ranges.is_empty() {
            return (" AND 0".into(), Vec::new());
        }
        let sql = format!(
            " AND ({})",
            vec!["(path_key >= ? AND path_key < ?)"; ranges.len()].join(" OR ")
        );
        (
            sql,
            ranges
                .into_iter()
                .flat_map(|(from, to)| [from, to])
                .collect(),
        )
    }

    // MARK: Files

    fn record(row: &rusqlite::Row) -> Result<FileRecord> {
        let kind: String = row.get(2)?;
        Ok(FileRecord {
            id: row.get(0)?,
            path: row.get(1)?,
            kind: FileKind::parse(&kind).unwrap_or(FileKind::Doc),
            size: row.get(3)?,
            mtime: row.get(4)?,
            inode: row.get::<_, i64>(5)? as u64,
            device: row.get::<_, i64>(6)? as u64,
            state: row.get(7)?,
        })
    }

    pub fn file(&self, path: &str) -> Result<Option<FileRecord>> {
        self.db
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files WHERE path = ?"),
                [path],
                Self::record,
            )
            .optional()
    }

    /// The file with this ID (volume serial and file reference), anywhere in the index.
    pub fn file_by_id(&self, device: u64, inode: u64) -> Result<Option<FileRecord>> {
        self.db
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files WHERE device = ? AND inode = ? LIMIT 1"),
                params![device as i64, inode as i64],
                Self::record,
            )
            .optional()
    }

    /// Every file under one of `roots`.
    pub fn files_under(&self, roots: &[String]) -> Result<Vec<FileRecord>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut statement = self.db.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE path_key >= ? AND path_key < ?"
        ))?;
        // A folder inside another one listed is already covered.
        for root in roots
            .iter()
            .filter(|root| !roots.iter().any(|other| is_inside(root, other)))
        {
            for (from, to) in path_ranges(root) {
                for record in statement.query_map([from, to], Self::record)? {
                    let record = record?;
                    if seen.insert(record.id) {
                        out.push(record);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Adds a file (pending) and returns its id.
    pub fn insert(&self, file: &Candidate) -> Result<i64> {
        self.db.execute(
            "INSERT INTO files(path, path_key, kind, size, mtime, inode, device) VALUES(?, ?, ?, ?, ?, ?, ?)",
            params![file.path, path_key(&file.path), file.kind.as_str(), file.size, file.mtime, file.inode as i64, file.device as i64],
        )?;
        let id = self.db.last_insert_rowid();
        self.add_name_row(id, &file.path)?;
        Ok(id)
    }

    /// The file changed on disk: drop what was extracted from it and queue it again.
    pub fn requeue_as(&self, id: i64, file: &Candidate) -> Result<()> {
        self.delete_segments(id, true)?;
        self.db.execute(
            "UPDATE files SET kind = ?, size = ?, mtime = ?, inode = ?, device = ?, state = 'pending', detail = NULL,
                info = NULL, indexed_at = NULL WHERE id = ?",
            params![file.kind.as_str(), file.size, file.mtime, file.inode as i64, file.device as i64, id],
        )?;
        Ok(())
    }

    /// Read and embed the file again, as it is.
    pub fn requeue(&self, id: i64) -> Result<()> {
        self.delete_segments(id, true)?;
        self.db.execute("UPDATE files SET state = 'pending', detail = NULL, info = NULL, indexed_at = NULL WHERE id = ?", [id])?;
        Ok(())
    }

    /// Same file ID, size and date at a new path: a move or rename. Only the path changes; nothing is re-embedded.
    pub fn move_to(&self, id: i64, path: &str) -> Result<()> {
        self.db.execute(
            "UPDATE files SET path = ?, path_key = ? WHERE id = ?",
            params![path, path_key(path), id],
        )?;
        self.db.execute(
            "UPDATE fts SET name = ?, body = ? WHERE rowid IN (SELECT id FROM segments WHERE file = ? AND kind = 'name')",
            params![normalized(&name_text(path)), normalized(&folder_text(path)), id],
        )?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.db.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM segments WHERE file = ?)",
            [id],
        )?;
        self.db.execute("DELETE FROM files WHERE id = ?", [id])?;
        Ok(())
    }

    /// Removes every file that isn't under one of `roots` (folders taken out of the app). Returns how many went.
    pub fn prune(&self, keeping_under: &[String]) -> Result<usize> {
        let prefixes: Vec<String> = keeping_under
            .iter()
            .flat_map(|root| path_ranges(root).into_iter().map(|(from, _)| from))
            .collect();
        let mut gone = Vec::new();
        {
            let mut statement = self.db.prepare("SELECT id, path_key FROM files")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (id, key) = row?;
                if !prefixes
                    .iter()
                    .any(|prefix| key.starts_with(prefix.as_str()))
                {
                    gone.push(id);
                }
            }
        }
        if gone.is_empty() {
            return Ok(0);
        }
        self.transaction(|| {
            for &id in &gone {
                self.delete(id)?;
            }
            Ok(())
        })?;
        Ok(gone.len())
    }

    /// Queues every file again and forgets the fingerprint (after the model or settings changed).
    pub fn reset_all(&self) -> Result<()> {
        self.transaction(|| {
            self.db.execute_batch(
                "DELETE FROM fts WHERE rowid IN (SELECT id FROM segments WHERE kind != 'name');
                 DELETE FROM segments WHERE kind != 'name';
                 UPDATE files SET state = 'pending', detail = NULL, info = NULL, indexed_at = NULL;",
            )?;
            self.set_meta("fingerprint", None)
        })
    }

    /// Files waiting to be embedded, in indexing order (screenshots first, then the newest first).
    pub fn pending(&self, limit: Option<usize>) -> Result<Vec<FileRecord>> {
        let (scope, mut values) = self.scope_sql();
        values.push(limit.map_or(-1, |n| n as i64).to_string());
        let mut statement = self.db.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE state = 'pending'{scope} ORDER BY {}, mtime DESC LIMIT ?",
            FileKind::INDEXING_ORDER_SQL
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), Self::record)?;
        rows.collect()
    }

    pub fn pending_count(&self) -> Result<i64> {
        let (scope, values) = self.scope_sql();
        self.db.query_row(
            &format!("SELECT COUNT(*) FROM files WHERE state = 'pending'{scope}"),
            rusqlite::params_from_iter(values.iter()),
            |row| row.get(0),
        )
    }

    /// Files read once more because Trovara reads them better now, each reason once per index (`flag` in meta): the
    /// files `ids_sql` selects go back to pending. Returns how many. (The schema stays as it is.)
    pub fn read_again_once(&self, flag: &str, ids_sql: &str) -> Result<usize> {
        if self.meta(flag)?.is_some() {
            return Ok(0);
        }
        let ids: Vec<i64> = {
            let mut statement = self.db.prepare(ids_sql)?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_>>()?
        };
        self.transaction(|| {
            for &id in &ids {
                self.requeue(id)?;
            }
            self.set_meta(flag, Some("1"))
        })?;
        Ok(ids.len())
    }

    /// Pictures that failed because no codec on this PC read them (the note names a Microsoft Store extension) go
    /// back to pending: a decode that fails again costs milliseconds, and once the extension is installed they're read.
    pub fn retry_missing_codecs(&self) -> Result<usize> {
        let ids: Vec<i64> = {
            let mut statement = self.db.prepare(
                "SELECT id FROM files WHERE state = 'failed' AND detail LIKE '%(Microsoft Store)%'",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<_>>()?
        };
        for &id in &ids {
            self.requeue(id)?;
        }
        Ok(ids.len())
    }

    /// Long files with pages or chunks still to read after their first step, in indexing order.
    pub fn unfinished(&self) -> Result<Vec<FileRecord>> {
        let (scope, values) = self.scope_sql();
        let mut statement = self.db.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE state = 'partial'{scope} ORDER BY {}, mtime DESC",
            FileKind::INDEXING_ORDER_SQL
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), Self::record)?;
        rows.collect()
    }

    /// The last page or chunk stored for a file (where a long file's reading got to, when its info doesn't say).
    pub fn last_place(&self, id: i64) -> Result<Option<usize>> {
        let last: Option<f64> = self.db.query_row(
            "SELECT MAX(loc) FROM segments WHERE file = ? AND kind IN ('page', 'page_image', 'chunk')",
            [id],
            |row| row.get(0),
        )?;
        Ok(last.map(|place| place as usize))
    }

    /// What indexing learned about a file.
    pub fn info(&self, id: i64) -> Result<Info> {
        let json: Option<String> = self
            .db
            .query_row("SELECT info FROM files WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .optional()?
            .flatten();
        Ok(json
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    pub fn finish(&self, id: i64, state: &str, detail: Option<&str>, info: &Info) -> Result<()> {
        let json = (!info.is_empty()).then(|| serde_json::to_string(info).unwrap_or_default());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        self.db.execute(
            "UPDATE files SET state = ?, detail = ?, info = ?, indexed_at = ? WHERE id = ?",
            params![state, detail, json, now, id],
        )?;
        Ok(())
    }

    /// A file the indexing helper had to stop on (reading it ran away): it isn't read again until it changes. A long
    /// file that was being finished keeps what was read of it. False when no file has that path.
    pub fn mark_stopped(&self, path: &str, why: &str) -> Result<bool> {
        let Some(found) = self.file(path)? else {
            return Ok(false);
        };
        if found.state == "partial" {
            self.finish(
                found.id,
                "done",
                Some(&format!("The rest couldn't be read: it {why}")),
                &self.info(found.id)?,
            )?;
        } else {
            self.finish(found.id, "failed", Some(&format!("It {why}")), &Info::new())?;
        }
        Ok(true)
    }

    // MARK: Segments

    /// Adds a segment: `text` goes into keyword search (normalized), `vector` into meaning search. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn add_segment(
        &self,
        file: i64,
        kind: SegmentKind,
        modality: Modality,
        loc: Option<f64>,
        loc_end: Option<f64>,
        excerpt: Option<&str>,
        text: Option<&str>,
        vector: Option<&[f32]>,
    ) -> Result<i64> {
        let modality = if vector.is_some() {
            modality
        } else {
            Modality::None
        };
        self.db.execute(
            "INSERT INTO segments(file, kind, modality, loc, loc_end, excerpt, vec) VALUES(?, ?, ?, ?, ?, ?, ?)",
            params![file, kind.as_str(), modality.as_str(), loc, loc_end, excerpt, vector.map(vectors::to_blob)],
        )?;
        let id = self.db.last_insert_rowid();
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            let (body, original) = keyword_text(text);
            self.db.execute(
                "INSERT INTO fts(rowid, name, body, original) VALUES(?, '', ?, ?)",
                params![id, body, original],
            )?;
        }
        Ok(id)
    }

    fn add_name_row(&self, file: i64, path: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO segments(file, kind, modality) VALUES(?, 'name', 'none')",
            [file],
        )?;
        let id = self.db.last_insert_rowid();
        self.db.execute(
            "INSERT INTO fts(rowid, name, body) VALUES(?, ?, ?)",
            params![
                id,
                normalized(&name_text(path)),
                normalized(&folder_text(path))
            ],
        )?;
        Ok(())
    }

    fn delete_segments(&self, file: i64, keeping_name: bool) -> Result<()> {
        let filter = if keeping_name {
            "AND kind != 'name'"
        } else {
            ""
        };
        self.db.execute(
            &format!(
                "DELETE FROM fts WHERE rowid IN (SELECT id FROM segments WHERE file = ? {filter})"
            ),
            [file],
        )?;
        self.db.execute(
            &format!("DELETE FROM segments WHERE file = ? {filter}"),
            [file],
        )?;
        Ok(())
    }

    /// Runs `body` in one transaction (much faster for many writes, and all or nothing).
    pub fn transaction<T>(&self, body: impl FnOnce() -> Result<T>) -> Result<T> {
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        match body() {
            Ok(value) => {
                self.db.execute_batch("COMMIT")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.db.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    // MARK: Stats

    pub fn counts(&self) -> Result<Counts> {
        let mut counts = Counts::default();
        let (scope, values) = self.scope_sql();
        {
            let mut statement = self.db.prepare(&format!(
                "SELECT kind, state, COUNT(*) FROM files WHERE 1{scope} GROUP BY kind, state"
            ))?;
            let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (kind, state, n) = row?;
                counts
                    .by_kind_and_state
                    .entry(kind)
                    .or_default()
                    .insert(state, n);
            }
        }
        (counts.unfinished, counts.left) = self.db.query_row(
            &format!(
                "SELECT COUNT(*), COALESCE(SUM(MAX(0, CAST(json_extract(info, '$.read_end') AS INTEGER)
                     - CAST(json_extract(info, '$.read_to') AS INTEGER))), 0)
                 FROM files WHERE state = 'partial'{scope}"
            ),
            rusqlite::params_from_iter(values.iter()),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let in_scope = if self.scope.is_some() {
            format!(" AND file IN (SELECT id FROM files WHERE 1{scope})")
        } else {
            String::new()
        };
        (counts.segments, counts.vectors) = self.db.query_row(
            &format!("SELECT COUNT(*), COUNT(vec) FROM segments WHERE kind != 'name'{in_scope}"),
            rusqlite::params_from_iter(values.iter()),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(counts)
    }
}

/// `body` and `original` for keyword search: the normalized text, and the text as written if that differs.
pub fn keyword_text(text: &str) -> (String, Option<String>) {
    let body = normalized(text);
    let original = (body != text).then(|| text.to_string());
    (body, original)
}

/// A path as the index compares it: Windows paths ignore case and take either slash, and a name typed with an accent
/// may arrive decomposed. NFC, lowercase, backslashes, no trailing backslash.
pub fn path_key(path: &str) -> String {
    let key: String = path
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .replace('/', "\\");
    match key.trim_end_matches('\\') {
        trimmed if trimmed.len() < key.len() && !trimmed.ends_with(':') => trimmed.to_string(),
        _ => key,
    }
}

/// The ranges of the `path_key` index that hold the files under `folder`.
pub fn path_ranges(folder: &str) -> Vec<(String, String)> {
    let key = path_key(folder);
    let prefix = if key.ends_with('\\') {
        key
    } else {
        format!("{key}\\")
    };
    let to = format!("{prefix}\u{10FFFF}");
    vec![(prefix, to)]
}

/// Whether `path` is `folder` or inside it (case-insensitively, either slash).
pub fn is_inside(path: &str, folder: &str) -> bool {
    let (path, folder) = (path_key(path), path_key(folder));
    path != folder && path.starts_with(&format!("{}\\", folder.trim_end_matches('\\')))
}

/// "Screenshot 2026-10-01 093107.png" → searchable words: the file name.
pub fn name_text(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

/// The folders a file sits in, relative to the home folder ("Downloads Invoices"), as low-weight keywords.
pub fn folder_text(path: &str) -> String {
    let folder = match path.rfind(['\\', '/']) {
        Some(at) => &path[..at],
        None => "",
    };
    let mut rest = folder;
    if let Some(home) = crate::paths::home().and_then(|home| home.to_str().map(str::to_string)) {
        if path_key(folder).starts_with(&path_key(&home)) && folder.len() >= home.len() {
            rest = &folder[home.len()..];
        }
    }
    rest.split(['\\', '/'])
        .filter(|part| !part.is_empty() && !part.ends_with(':'))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, inode: u64) -> Candidate {
        Candidate {
            path: path.into(),
            kind: FileKind::Image,
            size: 10,
            mtime: 1.0,
            inode,
            device: 1,
        }
    }

    fn fts_rows(store: &IndexStore) -> i64 {
        store
            .db
            .query_row("SELECT COUNT(*) FROM fts", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn fts5_has_the_tokenizer() {
        let store = IndexStore::in_memory().unwrap();
        let id = store.insert(&file(r"C:\notes\trip.txt", 1)).unwrap();
        store
            .add_segment(
                id,
                SegmentKind::Chunk,
                Modality::Text,
                Some(1.0),
                None,
                None,
                Some("কলকাতায় গিয়েছিলাম"),
                None,
            )
            .unwrap();
        let hits: i64 = store
            .db
            .query_row(
                "SELECT COUNT(*) FROM fts WHERE fts MATCH '\"কলকাতায়\"'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1, "Bengali words stay whole");
    }

    #[test]
    fn prune_keeps_only_chosen_folders() {
        let store = IndexStore::in_memory().unwrap();
        store.insert(&file(r"C:\a\x.png", 1)).unwrap();
        store.insert(&file(r"C:\a\sub\y.png", 2)).unwrap();
        store.insert(&file(r"C:\ab\z.png", 3)).unwrap(); // a sibling whose name starts like "a", not inside it
        store.insert(&file(r"C:\b\w.png", 4)).unwrap();
        assert_eq!(store.prune(&[r"C:\a".into()]).unwrap(), 2);
        assert_eq!(store.files_under(&[r"C:\a".into()]).unwrap().len(), 2);
        assert!(store
            .files_under(&[r"C:\ab".into(), r"C:\b".into()])
            .unwrap()
            .is_empty());
        assert_eq!(fts_rows(&store), 2); // the pruned files' names left keyword search too
        assert_eq!(store.prune(&[]).unwrap(), 2);
    }

    #[test]
    fn paths_ignore_case_and_slashes() {
        let store = IndexStore::in_memory().unwrap();
        store
            .insert(&file(r"C:\Users\Me\Pictures\Beach.JPG", 1))
            .unwrap();
        assert_eq!(
            store
                .files_under(&["c:/users/me/pictures/".into()])
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store.files_under(&[r"C:\USERS\ME".into()]).unwrap().len(),
            1
        );
        assert!(is_inside(r"C:\Users\Me\Pictures", r"c:\users\me"));
        assert!(!is_inside(r"C:\Users\Media", r"C:\Users\Me"));
        assert_eq!(path_key(r"C:\"), r"c:\");
        assert_eq!(name_text(r"C:\Users\Me\Pictures\Beach.JPG"), "Beach.JPG");
    }

    #[test]
    fn scope_limits_what_is_waiting() {
        let mut store = IndexStore::in_memory().unwrap();
        store.insert(&file(r"C:\a\x.png", 1)).unwrap();
        store.insert(&file(r"E:\drive\y.png", 2)).unwrap(); // on a drive that's unplugged
        store.set_scope(Some(vec![r"C:\a".into()]));
        assert_eq!(store.pending_count().unwrap(), 1);
        assert_eq!(
            store
                .pending(None)
                .unwrap()
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            [r"C:\a\x.png"]
        );
        assert_eq!(
            store.counts().unwrap().by_kind_and_state["image"]["pending"],
            1
        );
        store.set_scope(Some(vec![]));
        assert_eq!(store.pending_count().unwrap(), 0);
        store.set_scope(None);
        assert_eq!(store.pending_count().unwrap(), 2);
    }

    #[test]
    fn a_file_the_helper_stopped_on_is_not_read_again() {
        let store = IndexStore::in_memory().unwrap();
        let id = store.insert(&file(r"C:\a\huge.pdf", 1)).unwrap();
        assert!(store
            .mark_stopped(r"C:\a\huge.pdf", "used too much memory")
            .unwrap());
        assert_eq!(store.pending_count().unwrap(), 0);
        let state: String = store
            .db
            .query_row("SELECT state FROM files WHERE id = ?", [id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(state, "failed");
        assert!(!store.mark_stopped(r"C:\a\other.pdf", "x").unwrap());
    }

    #[test]
    fn moves_keep_segments_and_requeue_drops_them() {
        let store = IndexStore::in_memory().unwrap();
        let id = store.insert(&file(r"C:\a\IMG_1.png", 1)).unwrap();
        store
            .add_segment(
                id,
                SegmentKind::Image,
                Modality::Image,
                None,
                None,
                None,
                None,
                Some(&vec![0.0; 768]),
            )
            .unwrap();
        store.move_to(id, r"C:\b\beach day.png").unwrap();
        assert_eq!(store.counts().unwrap().vectors, 1);
        let name: String = store
            .db
            .query_row(
                "SELECT name FROM fts JOIN segments s ON s.id = fts.rowid WHERE s.file = ?",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "beach day.png");
        store.requeue(id).unwrap();
        assert_eq!(store.counts().unwrap().vectors, 0);
        assert_eq!(fts_rows(&store), 1); // the name row stays
        store.delete(id).unwrap();
        assert_eq!(fts_rows(&store), 0);
    }
}
