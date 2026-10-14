//! The vectors this build makes for fixed inputs, against the stored ones `VECTOR_VERSION` stands for
//! (tests/fixtures/reference/vectors.json, made with llama.cpp b11461 on the reference macOS runtime, Metal). Every
//! llama.cpp update must pass this before it ships: indexes record the version, not the llama.cpp build.
//!
//! Needs the model files (trovara-helper download-model); without them the tests say so and pass.

use trovara_core::Prompts;
use trovara_llama::reference::{self, cosine};
use trovara_llama::{Embedder, Models, AGREEMENT};

#[test]
fn vectors_match_the_reference() {
    let models = match Models::locate() {
        Ok(models) if models.media.is_some() => models,
        _ => {
            eprintln!("SKIPPED: the model isn't downloaded (trovara-helper download-model)");
            return;
        }
    };
    let folder = reference::folder().expect("tests/fixtures/reference");
    let log = std::env::temp_dir().join("trovara-reference-vectors.log");
    let mut embedder = Embedder::new(&models, false, Some(&log)).expect("the embedder loads");
    let stored = reference::stored(&folder).unwrap();
    let made = reference::make(&mut embedder, &folder).unwrap();

    assert_eq!(
        made.keys().collect::<Vec<_>>(),
        stored.keys().collect::<Vec<_>>()
    );
    let mut failures = Vec::new();
    for (name, vector) in &made {
        assert_eq!(vector.len(), 768, "{name}");
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-3,
            "{name} isn't normalized ({norm})"
        );
        let similarity = cosine(vector, &stored[name]);
        println!("{name:14} cosine {similarity:.5}");
        if similarity < AGREEMENT {
            failures.push(format!("{name}: {similarity:.5}"));
        }
    }
    println!("devices: {}", trovara_llama::devices().join("; "));
    assert!(
        failures.is_empty(),
        "vectors moved from the reference (< {AGREEMENT}): {}. Keep the old llama.cpp, or bump VECTOR_VERSION.",
        failures.join(", ")
    );
}

#[test]
fn texts_batch_like_one_at_a_time() {
    let Ok(models) = Models::locate() else {
        eprintln!("SKIPPED: the model isn't downloaded");
        return;
    };
    let mut embedder = Embedder::new(&models, true, None).expect("the text model loads");
    let queries: Vec<String> = [
        "a dog on the beach",
        "payment declined error",
        "where they talk about sleep",
    ]
    .iter()
    .map(|q| Prompts::query(q))
    .collect();
    let together = embedder.embed_texts(&queries).unwrap();
    for (query, vector) in queries.iter().zip(&together) {
        let alone = embedder.embed_texts(&[query]).unwrap().remove(0);
        assert!(cosine(&alone, vector) > 0.9999, "batching changed {query}");
    }
    // Different meanings stay apart.
    assert!(cosine(&together[0], &together[1]) < 0.9);
}

#[test]
fn cross_language_queries_meet() {
    let Ok(models) = Models::locate() else {
        eprintln!("SKIPPED: the model isn't downloaded");
        return;
    };
    let mut embedder = Embedder::new(&models, true, None).expect("the text model loads");
    let english = embedder
        .embed_texts(&[Prompts::query("a dog on the beach")])
        .unwrap()
        .remove(0);
    let documents = embedder
        .embed_texts(&[
            Prompts::document(None, "কুকুরটি সমুদ্র সৈকতে বালিতে দৌড়াচ্ছিল।"), // Bengali: the dog ran on the sand at the beach
            Prompts::document(
                None,
                "The quarterly invoice is due on the fifth of next month.",
            ),
        ])
        .unwrap();
    assert!(
        cosine(&english, &documents[0]) > cosine(&english, &documents[1]),
        "an English query should find the Bengali note about a dog on a beach before an unrelated English one"
    );
}
