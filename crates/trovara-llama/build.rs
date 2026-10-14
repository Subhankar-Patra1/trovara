//! Compiles the C shim against llama.cpp's headers and links the import libraries that scripts/build-llama.ps1 put in
//! vendor/llama. For dev runs and tests, the DLLs (llama, mtmd, ggml and its backends) are copied beside the binaries
//! cargo builds, where Windows looks for them first.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let llama = std::env::var_os("TROVARA_LLAMA")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("vendor").join("llama"));
    let include = llama.join("include");
    if !include.join("llama.h").is_file() {
        panic!(
            "llama.cpp isn't built: no {}. Run scripts\\build-llama.ps1 first.",
            include.join("llama.h").display()
        );
    }

    cc::Build::new()
        .file(manifest.join("shim").join("trovara_llama.c"))
        .include(&include)
        .warnings(false)
        .compile("trovara_llama_shim");
    println!("cargo:rerun-if-changed=shim/trovara_llama.c");
    println!("cargo:rerun-if-changed={}", llama.join("BUILD").display());
    println!("cargo:rerun-if-env-changed=TROVARA_LLAMA");

    println!(
        "cargo:rustc-link-search=native={}",
        llama.join("lib").display()
    );
    for library in ["llama", "mtmd", "ggml", "ggml-base"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }

    // target/<profile>/build/<crate>-<hash>/out → target/<profile>, plus deps/ (test binaries) and examples/.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    if let Some(profile) = out.ancestors().nth(3) {
        let bin = llama.join("bin");
        for folder in [
            profile.to_path_buf(),
            profile.join("deps"),
            profile.join("examples"),
        ] {
            let _ = std::fs::create_dir_all(&folder);
            if let Ok(entries) = std::fs::read_dir(&bin) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
                    {
                        let target = folder.join(entry.file_name());
                        let stale = match (std::fs::metadata(&path), std::fs::metadata(&target)) {
                            (Ok(source), Ok(copy)) => {
                                source.len() != copy.len()
                                    || source.modified().ok() > copy.modified().ok()
                            }
                            _ => true,
                        };
                        if stale {
                            let _ = std::fs::copy(&path, &target);
                        }
                    }
                }
            }
        }
    }
}
