//! Links PDFium's import library (scripts/fetch-vendor.ps1 puts the prebuilt DLL in vendor/pdfium) and, for dev runs
//! and tests, copies pdfium.dll beside the binaries cargo builds.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let pdfium = std::env::var_os("TROVARA_PDFIUM")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("vendor").join("pdfium"));
    let dll = pdfium.join("bin").join("pdfium.dll");
    if !dll.is_file() {
        panic!(
            "PDFium isn't there: no {}. Run scripts\\fetch-vendor.ps1 first.",
            dll.display()
        );
    }
    println!("cargo:rerun-if-env-changed=TROVARA_PDFIUM");
    println!("cargo:rerun-if-changed={}", dll.display());
    println!(
        "cargo:rustc-link-search=native={}",
        pdfium.join("lib").display()
    );
    println!("cargo:rustc-link-lib=dylib=pdfium.dll"); // pdfium.dll.lib

    // target/<profile>/build/<crate>-<hash>/out → target/<profile>, plus deps/ (test binaries).
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    if let Some(profile) = out.ancestors().nth(3) {
        for folder in [profile.to_path_buf(), profile.join("deps")] {
            let _ = std::fs::create_dir_all(&folder);
            let target = folder.join("pdfium.dll");
            let stale = match (std::fs::metadata(&dll), std::fs::metadata(&target)) {
                (Ok(source), Ok(copy)) => source.len() != copy.len(),
                _ => true,
            };
            if stale {
                let _ = std::fs::copy(&dll, &target);
            }
        }
    }
}
