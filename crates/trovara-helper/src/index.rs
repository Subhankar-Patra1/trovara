//! `search` and `status`: the index from the command line (TRD F-CLI), and the argument helpers the commands share.
//! Indexing is in `indexer.rs`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use trovara_core::searcher::{strong_count, SearchOptions};
use trovara_core::{paths, CodeQuery, FileKind, IndexStore, Prompts, Searcher, SegmentKind};
use trovara_llama::{Embedder, Models};

use crate::{flag, value};

pub fn index_folder(args: &[String]) -> PathBuf {
    value(args, "--index")
        .map(PathBuf::from)
        .unwrap_or_else(paths::index)
}

/// Every value given for an option that can repeat (`--exclude A --exclude B`).
pub fn values(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .collect()
}

/// The positional arguments (not options or their values).
pub fn positional(args: &[String], options_with_values: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
        } else if options_with_values.contains(&arg.as_str()) || arg == "-n" {
            skip = true;
        } else if !arg.starts_with("--") {
            out.push(arg.clone());
        }
    }
    out
}

pub fn search(args: &[String]) -> Result<(), String> {
    let mut query =
        positional(args, &["--index", "--kinds", "--z-gate", "--word-weight"]).join(" ");
    if query.trim().is_empty() {
        return Err("search for what?".into());
    }
    let code = CodeQuery::text(&query).map(str::to_string);
    if let Some(code) = &code {
        query = code.clone();
    }
    let file = if code.is_some() {
        paths::CODE_INDEX_FILE
    } else {
        paths::MAIN_INDEX_FILE
    };
    let store = IndexStore::open(&index_folder(args), file).map_err(|e| e.to_string())?;
    let mut searcher =
        Searcher::new(store, !flag(args, "--words-only")).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let vector = if flag(args, "--words-only") || searcher.vector_count() == 0 {
        None
    } else {
        let models = Models::locate().map_err(|e| e.to_string())?;
        let mut encoder = Embedder::new(&models, true, Some(&paths::logs().join("search.log")))
            .map_err(|e| e.to_string())?;
        let prompt = if code.is_some() {
            Prompts::code_query(&query)
        } else {
            Prompts::query(&query)
        };
        Some(
            encoder
                .embed_texts(&[prompt])
                .map_err(|e| e.to_string())?
                .remove(0),
        )
    };
    let encoded = Instant::now();
    let kinds: Option<BTreeSet<FileKind>> = value(args, "--kinds").map(|list| {
        list.split(',')
            .filter_map(|k| FileKind::parse(k.trim()))
            .collect()
    });
    let limit = value(args, "-n").and_then(|n| n.parse().ok()).unwrap_or(10);
    // For evals and tuning, as the reference's CLI: how far a match must stand out, and how much words weigh.
    let z_gate = value(args, "--z-gate")
        .and_then(|z| z.parse().ok())
        .unwrap_or(trovara_core::searcher::Z_GATE);
    let word_weight = value(args, "--word-weight").and_then(|w| w.parse().ok());
    let hits = searcher
        .search(
            &query,
            vector.as_deref(),
            &SearchOptions {
                kinds: kinds.as_ref(),
                limit,
                z_gate,
                word_weight,
                ..SearchOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
    let searched = Instant::now();
    if flag(args, "--json") {
        let strong = strong_count(&hits, 0.5);
        println!(
            "{}",
            serde_json::json!({"hits": hits, "strong": strong, "meaning": vector.is_some(),
            "ms": {"embed": (encoded - started).as_secs_f64() * 1000.0, "search": (searched - encoded).as_secs_f64() * 1000.0}})
        );
        return Ok(());
    }
    if hits.is_empty() {
        println!("no good matches");
    }
    for (rank, hit) in hits.iter().enumerate() {
        let why: Vec<String> = [
            hit.z.map(|z| format!("z {z:.1}")),
            hit.keyword_rank.map(|r| format!("words #{r}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let place = location(hit);
        let name = Path::new(&hit.path)
            .file_name()
            .map_or(hit.path.clone(), |n| n.to_string_lossy().into_owned());
        let folder = Path::new(&hit.path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        println!(
            "{:>2}. {:10} {name}{}   ({})",
            rank + 1,
            hit.kind.as_str(),
            if place.is_empty() {
                String::new()
            } else {
                format!("  [{place}]")
            },
            why.join(", ")
        );
        println!("      {folder}");
        if let Some(excerpt) = hit.excerpt.as_ref().filter(|e| !e.is_empty()) {
            println!("      “{}”", excerpt.chars().take(160).collect::<String>());
        }
    }
    println!(
        "({} vectors · query encoder {:.2} s incl. start-up · search {:.1} ms)",
        searcher.vector_count(),
        (encoded - started).as_secs_f64(),
        (searched - encoded).as_secs_f64() * 1000.0
    );
    Ok(())
}

fn clock(seconds: f64) -> String {
    let s = seconds as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn location(hit: &trovara_core::SearchHit) -> String {
    let Some(loc) = hit.loc else {
        return if hit.segment == SegmentKind::Ocr {
            "text in image".into()
        } else {
            String::new()
        };
    };
    match hit.segment {
        SegmentKind::Page | SegmentKind::PageImage => format!("p. {}", loc as i64),
        SegmentKind::Audio => format!("at {} (sound)", clock(loc)),
        SegmentKind::Frame => format!("at {}", clock(loc)),
        SegmentKind::Subtitle => format!("said at {}", clock(loc)),
        SegmentKind::Chunk => format!("part {}", loc as i64),
        SegmentKind::Lines | SegmentKind::Cells => {
            let what = if hit.segment == SegmentKind::Cells { "cells" } else { "lines" };
            let span = format!("{what} {}–{}", loc as i64, hit.loc_end.unwrap_or(loc) as i64);
            match hit.focus {
                Some(line) => format!("{span}, opens at {}", line as i64),
                None => span,
            }
        }
        SegmentKind::Ocr => "text in image".into(),
        _ => String::new(),
    }
}

pub fn status(args: &[String]) -> Result<(), String> {
    let store =
        IndexStore::open(&index_folder(args), paths::MAIN_INDEX_FILE).map_err(|e| e.to_string())?;
    let counts = store.counts().map_err(|e| e.to_string())?;
    if flag(args, "--json") {
        println!(
            "{}",
            serde_json::json!({"index": store.file.display().to_string(), "vectors": counts.vectors, "segments": counts.segments,
            "files": counts.by_kind_and_state, "unfinished": counts.unfinished, "left": counts.left, "bytes": store.size_on_disk()})
        );
        return Ok(());
    }
    println!("index: {}", store.file.display());
    println!(
        "vectors: {} in {} segments",
        counts.vectors, counts.segments
    );
    for kind in FileKind::ALL {
        let Some(states) = counts.by_kind_and_state.get(kind.as_str()) else {
            continue;
        };
        let total: i64 = states.values().sum();
        let line: Vec<String> = states
            .iter()
            .map(|(state, n)| format!("{n} {state}"))
            .collect();
        println!("  {:11} {total:>6}   {}", kind.as_str(), line.join(" · "));
    }
    if let Some(fingerprint) = store.meta("fingerprint").map_err(|e| e.to_string())? {
        println!("built with: {fingerprint}");
    }
    println!("size: {:.1} MB", store.size_on_disk() as f64 / 1e6);
    Ok(())
}
