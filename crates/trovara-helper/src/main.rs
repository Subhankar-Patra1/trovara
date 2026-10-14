//! `trovara-helper`: does all of Trovara's heavy work, as a CLI for developers and as long-running services the tray
//! app talks to over JSON lines (TRD §10.2, §11). Commands fill in milestone by milestone.

mod crawler;
mod download;
mod estimator;
mod eval;
mod index;
mod indexer;
mod search_service;
mod service;
mod usn;
mod watchdog;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use trovara_core::Prompts;
use trovara_llama::{Embedder, Models};

const USAGE: &str = "\
trovara-helper <command> [options]

Commands:
  download-model [--json] [--from DIR]   Download EmbeddingGemma 2 (865 MB, resumable, checksummed)
  embed (--text T | --query Q | --image PATH [--budget N] | --audio PATH) [--repeat N] [--json]
                                         Embed one input (developer check)
  devices                                The compute devices llama.cpp finds (CPU, Vulkan, CUDA)
  self-check [--cpu] [--json]            Embed the reference inputs and compare with the stored vectors
  estimate <folders> [--json]            What indexing these folders would take
  index <folders> [--index DIR]          Index folders: pictures, PDFs, documents (audio and video from M5)
  search <query> [--kinds K,K] [-n N] [--words-only] [--json] [--index DIR]
                                         Search the index (start with code: for the code index)
  status [--json] [--index DIR]          What's in the index
  speed                                  Measure this PC (embedding, OCR)
  eval <evals/*.json>                    Run the ranking evals (R@1, R@5, MRR)
  index-service [options] <roots>        Keep the index current (for the app)
  search-service [options]               Answer the search panel (for the app)
  paths                                  Where Trovara keeps its files
  version                                Versions of Trovara, llama.cpp and the vector pipeline";

fn main() -> ExitCode {
    trovara_win::system::harden_dll_search();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = args.first() else {
        println!("{USAGE}");
        return ExitCode::from(64);
    };
    let rest = &args[1..];
    let result = match command.as_str() {
        "paths" => {
            println!("data     {}", trovara_core::paths::data_root().display());
            println!("models   {}", trovara_core::paths::models().display());
            println!("index    {}", trovara_core::paths::index().display());
            println!("logs     {}", trovara_core::paths::logs().display());
            println!(
                "settings {}",
                trovara_core::paths::settings_file().display()
            );
            Ok(())
        }
        "version" => {
            println!(
                "trovara-helper {} · llama.cpp {} · vectors {}",
                env!("CARGO_PKG_VERSION"),
                trovara_llama::LLAMA_TAG,
                trovara_llama::VECTOR_VERSION
            );
            Ok(())
        }
        "download-model" => download_model(rest),
        "embed" => embed(rest),
        "devices" => devices(),
        "self-check" => self_check(rest),
        "index" => indexer::index(rest),
        "search" => index::search(rest),
        "status" => index::status(rest),
        "index-service" => service::serve(rest),
        "usn" => usn::command(rest),
        "estimate" => estimator::run(rest),
        "speed" => estimator::speed(rest),
        "search-service" => search_service::serve(rest),
        "eval" => eval::run(rest),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n\n{USAGE}");
            return ExitCode::from(64);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            if rest.iter().any(|arg| arg == "--json") {
                println!(
                    "{}",
                    serde_json::json!({"event": "error", "message": message})
                );
            } else {
                eprintln!("error: {message}");
            }
            ExitCode::FAILURE
        }
    }
}

/// The value after `--name`, if given.
pub(crate) fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

pub(crate) fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn download_model(args: &[String]) -> Result<(), String> {
    let folder = std::env::var_os("TROVARA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(trovara_core::paths::models);
    download::run(&download::Options {
        json: flag(args, "--json"),
        folder,
        from: value(args, "--from").map(PathBuf::from),
    })
}

fn devices() -> Result<(), String> {
    let models = Models::locate().map_err(|error| error.to_string())?;
    // Loading the text model initializes the backends, so this shows what embedding would actually use.
    let embedder = Embedder::new(&models, true, None).map_err(|error| error.to_string())?;
    for device in trovara_llama::devices() {
        println!("{device}");
    }
    println!(
        "chosen: {} · threads {} · text model ready in {} s",
        embedder.device,
        trovara_llama::threads(),
        embedder.load_seconds
    );
    Ok(())
}

/// The reference inputs on this PC's backend (or the CPU with `--cpu`), against the stored vectors: what the app's
/// GPU self-check runs, shown in full.
fn self_check(args: &[String]) -> Result<(), String> {
    let models = Models::locate().map_err(|error| error.to_string())?;
    let folder = trovara_llama::reference::folder()
        .ok_or("no reference folder (reference\\vectors.json beside the helper)")?;
    let started = Instant::now();
    let mut embedder = Embedder::with_gpu(&models, false, None, !flag(args, "--cpu"))
        .map_err(|error| error.to_string())?;
    let check = trovara_llama::reference::check(&mut embedder, &folder)
        .map_err(|error| error.to_string())?;
    let seconds = started.elapsed().as_secs_f64();
    if flag(args, "--json") {
        let cosines: serde_json::Map<String, serde_json::Value> = check
            .cosines
            .iter()
            .map(|(name, cosine)| (name.clone(), serde_json::json!(cosine)))
            .collect();
        println!(
            "{}",
            serde_json::json!({"passed": check.passed(), "cosines": cosines, "devices": trovara_llama::devices(), "device": embedder.device, "seconds": seconds})
        );
    } else {
        for (name, cosine) in &check.cosines {
            println!(
                "{name:14} {cosine:.5}{}",
                if *cosine < trovara_llama::AGREEMENT {
                    "  ← below 0.999"
                } else {
                    ""
                }
            );
        }
        println!("devices: {}", trovara_llama::devices().join("; "));
        println!("ran on: {}", embedder.device);
        println!(
            "{} in {seconds:.1} s",
            if check.passed() { "PASSED" } else { "FAILED" }
        );
    }
    if check.passed() {
        Ok(())
    } else {
        Err("the vectors differ from the reference on this backend".into())
    }
}

fn embed(args: &[String]) -> Result<(), String> {
    let models = Models::locate().map_err(|error| error.to_string())?;
    let text_only = value(args, "--image").is_none() && value(args, "--audio").is_none();
    let mut embedder =
        Embedder::new(&models, text_only, None).map_err(|error| error.to_string())?;
    let repeat: usize = value(args, "--repeat")
        .and_then(|n| n.parse().ok())
        .unwrap_or(1)
        .max(1);
    let mut first_ms = 0.0;
    let mut best_ms = f64::MAX;
    let mut vector = Vec::new();
    for round in 0..repeat {
        let started = Instant::now();
        vector = if let Some(text) = value(args, "--query") {
            embedder
                .embed_texts(&[Prompts::query(&text)])
                .map(|mut v| v.remove(0))
        } else if let Some(text) = value(args, "--text") {
            embedder
                .embed_texts(&[Prompts::document(None, &text)])
                .map(|mut v| v.remove(0))
        } else if let Some(path) = value(args, "--image") {
            let budget = value(args, "--budget")
                .and_then(|b| b.parse().ok())
                .unwrap_or(280);
            embedder.embed_image_file(std::path::Path::new(&path), budget)
        } else if let Some(path) = value(args, "--audio") {
            embedder.embed_audio_file(std::path::Path::new(&path))
        } else {
            return Err("say what to embed: --query, --text, --image or --audio".into());
        }
        .map_err(|error| error.to_string())?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        if round == 0 {
            first_ms = elapsed;
        }
        best_ms = best_ms.min(elapsed);
    }
    let ms = best_ms;
    if flag(args, "--json") {
        println!(
            "{}",
            serde_json::json!({"dim": vector.len(), "ms": ms, "load_s": embedder.load_seconds, "vector": vector})
        );
    } else {
        let head: Vec<String> = vector.iter().take(8).map(|x| format!("{x:+.4}")).collect();
        if repeat > 1 {
            println!(
                "dim {} · {:.1} ms (first {:.0} ms) on {} (model loaded in {} s)",
                vector.len(),
                ms,
                first_ms,
                embedder.device,
                embedder.load_seconds
            );
        } else {
            println!(
                "dim {} · {:.1} ms on {} (model loaded in {} s)",
                vector.len(),
                ms,
                embedder.device,
                embedder.load_seconds
            );
        }
        println!("[{} …]", head.join(", "));
    }
    Ok(())
}
