//! `trovara-helper eval <queries.json> [-n 10] [--index DIR] [--z-gate Z] [--word-weight W]`: recall of known answers
//! (evals\, TRD §15), as the reference measures it. "code: …" queries search the code index with the code prompt, the
//! others the main one. An expected "path@6-12" also needs the hit at that place: a page or a moment starting in the
//! range, a stretch of code overlapping it. TROVARA_TESTBED stands for ~/DigUpTestbed in the file's base.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Deserialize;
use trovara_core::searcher::{strong_count, SearchOptions, Z_GATE};
use trovara_core::store::path_key;
use trovara_core::{paths, CodeQuery, FileKind, IndexStore, Prompts, SearchHit, Searcher, SegmentKind};
use trovara_llama::{Embedder, Models};

use crate::value;

#[derive(Deserialize)]
struct EvalFile {
    base: String,
    queries: Vec<Query>,
}

#[derive(Deserialize)]
struct Query {
    q: String,
    /// Paths relative to `base`; "path@6-12" also needs the hit at that place.
    expect: Vec<String>,
    tag: Option<String>,
    kind: Option<String>,
}

#[derive(Default)]
struct Row {
    n: usize,
    at1: usize,
    at5: usize,
    at10: usize,
    mrr: f64,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let file = crate::index::positional(args, &["--index", "--z-gate", "--word-weight"])
        .into_iter()
        .next()
        .ok_or("which eval file? trovara-helper eval evals\\code.json")?;
    let eval: EvalFile = serde_json::from_slice(&std::fs::read(&file).map_err(|e| format!("{file}: {e}"))?)
        .map_err(|e| format!("{file}: {e}"))?;
    let base = expand_base(&eval.base);
    let depth: usize = value(args, "-n").and_then(|n| n.parse().ok()).unwrap_or(10);
    let z_gate = value(args, "--z-gate").and_then(|z| z.parse().ok()).unwrap_or(Z_GATE);
    let word_weight = value(args, "--word-weight").and_then(|w| w.parse().ok());
    let index_folder = crate::index::index_folder(args);

    let is_code: Vec<bool> = eval.queries.iter().map(|q| CodeQuery::text(&q.q).is_some()).collect();
    let texts: Vec<String> = eval.queries.iter().map(|q| CodeQuery::text(&q.q).unwrap_or(&q.q).trim().to_string()).collect();
    let mut searchers: HashMap<bool, Searcher> = HashMap::new();
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
    let mut encoder: Option<Embedder> = None;
    for code in is_code.iter().copied().collect::<BTreeSet<_>>() {
        let store = IndexStore::open(&index_folder, if code { paths::CODE_INDEX_FILE } else { paths::MAIN_INDEX_FILE })
            .map_err(|e| e.to_string())?;
        let searcher = Searcher::new(store, true).map_err(|e| e.to_string())?;
        if searcher.vector_count() > 0 {
            if encoder.is_none() {
                let models = Models::locate().map_err(|e| e.to_string())?;
                encoder = Some(Embedder::new(&models, true, Some(&paths::logs().join("eval.log"))).map_err(|e| e.to_string())?);
            }
            let indices: Vec<usize> = (0..texts.len()).filter(|&i| is_code[i] == code).collect();
            for batch in indices.chunks(16) {
                let prompts: Vec<String> = batch
                    .iter()
                    .map(|&i| if code { Prompts::code_query(&texts[i]) } else { Prompts::query(&texts[i]) })
                    .collect();
                let made = encoder.as_mut().unwrap().embed_texts(&prompts).map_err(|e| e.to_string())?;
                for (&i, vector) in batch.iter().zip(made) {
                    vectors[i] = Some(vector);
                }
            }
        }
        searchers.insert(code, searcher);
    }

    let mut by_tag: BTreeMap<String, Row> = BTreeMap::new();
    let (mut shown, mut listed, mut folded) = (0usize, 0usize, 0usize);
    for (index, query) in eval.queries.iter().enumerate() {
        let kinds: Option<BTreeSet<FileKind>> = query.kind.as_deref().and_then(FileKind::parse).map(|k| [k].into());
        let searcher = searchers.get_mut(&is_code[index]).unwrap();
        let options = SearchOptions { kinds: kinds.as_ref(), limit: depth, z_gate, word_weight, ..SearchOptions::default() };
        let hits = searcher.search(&texts[index], vectors[index].as_deref(), &options).map_err(|e| e.to_string())?;
        let strong = strong_count(&hits, 0.5);
        shown += strong;
        listed += hits.len();
        let targets: Vec<(String, Option<(f64, f64)>)> = query
            .expect
            .iter()
            .map(|entry| {
                let (path, range) = entry.split_once('@').map_or((entry.as_str(), None), |(p, r)| (p, Some(r)));
                let range = range.and_then(|r| {
                    let (a, b) = r.split_once('-')?;
                    Some((a.parse().ok()?, b.parse().ok()?))
                });
                (path_key(&format!("{base}\\{}", path.replace('/', "\\"))), range)
            })
            .collect();
        let rank = hits
            .iter()
            .position(|hit| {
                targets.iter().any(|(path, range)| {
                    path_key(&hit.path) == *path && range.is_none_or(|range| at(hit, range))
                })
            })
            .map(|i| i + 1);
        if rank.is_some_and(|rank| rank > strong) {
            folded += 1;
        }
        let tag = query.tag.clone().unwrap_or_else(|| "all".into());
        for key in [tag.clone(), "ALL".to_string()] {
            let row = by_tag.entry(key).or_default();
            row.n += 1;
            if let Some(rank) = rank {
                row.at1 += (rank <= 1) as usize;
                row.at5 += (rank <= 5) as usize;
                row.at10 += (rank <= 10) as usize;
                row.mrr += 1.0 / rank as f64;
            }
        }
        let top = hits.first().map_or("-".to_string(), |hit| {
            let name = std::path::Path::new(&hit.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match place(hit) {
                Some(place) => format!("{name} [{place}]"),
                None => name,
            }
        });
        let shown_rank = rank.map_or("miss".to_string(), |r| format!("#{r}"));
        let quoted = format!("“{}”", query.q);
        println!("{shown_rank:>4}  {tag:<11} {quoted:<62} top: {top}");
    }
    println!();
    println!("{:<12} {:>3}  {:>5}  {:>5}  {:>5}  {:>5}", "tag", "n", "R@1", "R@5", "R@10", "MRR");
    let mut tags: Vec<(&String, &Row)> = by_tag.iter().collect();
    tags.sort_by_key(|(tag, _)| if *tag == "ALL" { "~".to_string() } else { tag.to_string() });
    for (tag, row) in tags {
        let n = row.n as f64;
        println!(
            "{tag:<12} {:>3}  {:5.2}  {:5.2}  {:5.2}  {:5.2}",
            row.n,
            row.at1 as f64 / n,
            row.at5 as f64 / n,
            row.at10 as f64 / n,
            row.mrr / n
        );
    }
    let count = eval.queries.len().max(1) as f64;
    println!(
        "weaker-matches fold (below 0.5× the top score): {:.1} of {:.1} results shown per query; {folded} expected {} folded",
        shown as f64 / count,
        listed as f64 / count,
        if folded == 1 { "answer" } else { "answers" }
    );
    Ok(())
}

/// A place to be at: a page or moment starts in the range; a stretch of code overlaps it (its lines or cells).
fn at(hit: &SearchHit, (low, high): (f64, f64)) -> bool {
    let Some(loc) = hit.loc else { return false };
    if matches!(hit.segment, SegmentKind::Lines | SegmentKind::Cells) {
        return loc <= high && hit.loc_end.unwrap_or(loc) >= low;
    }
    (low..=high).contains(&loc)
}

fn place(hit: &SearchHit) -> Option<String> {
    let loc = hit.loc?;
    Some(match hit.segment {
        SegmentKind::Lines | SegmentKind::Cells => format!("{}–{}", loc as i64, hit.loc_end.unwrap_or(loc) as i64),
        SegmentKind::Page | SegmentKind::PageImage => format!("p. {}", loc as i64),
        _ => format!("{loc:.0} s"),
    })
}

/// The eval's base as a Windows path: "~" is the profile folder, and TROVARA_TESTBED stands for ~/DigUpTestbed.
fn expand_base(base: &str) -> String {
    if let (Some(testbed), Some(rest)) = (std::env::var_os("TROVARA_TESTBED"), base.strip_prefix("~/DigUpTestbed")) {
        return format!("{}{}", testbed.to_string_lossy().trim_end_matches(['\\', '/']), rest.replace('/', "\\"));
    }
    if let (Some(home), Some(rest)) = (std::env::var_os("USERPROFILE"), base.strip_prefix('~')) {
        return format!("{}{}", home.to_string_lossy(), rest.replace('/', "\\"));
    }
    base.replace('/', "\\")
}
