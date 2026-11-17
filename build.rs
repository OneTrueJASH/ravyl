//! Compiles the vendored RefEngine C engine into a static library when the
//! `refengine` feature is on (it is off by default). RefEngine is only a
//! reference in tests, for move generation and rack inference; the app
//! builds and runs without it.

#[cfg(feature = "refengine")]
use std::env;
#[cfg(feature = "refengine")]
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "refengine")]
    build_refengine();
}

#[cfg(feature = "refengine")]
fn collect_c_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_c_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "c") {
            out.push(path);
        }
    }
}

#[cfg(feature = "refengine")]
fn build_refengine() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_dir = manifest_dir.join("external/refengine-src/src");
    println!("cargo:rerun-if-changed={}", src_dir.display());

    let mut sources = Vec::new();
    collect_c_sources(&src_dir, &mut sources);
    if sources.is_empty() {
        panic!(
            "RefEngine sources not found in {}.\n\
             Run `git submodule update --init` to fetch them, or build with \
             `--no-default-features` to leave RefEngine out.",
            src_dir.display()
        );
    }

    let mut build = cc::Build::new();
    build
        .files(&sources)
        .include(&src_dir)
        .define("BOARD_DIM", "15")
        .define("RACK_SIZE", "7")
        .define("NDEBUG", None)
        .opt_level(3)
        // RefEngine is built with -Werror upstream; warnings from a different
        // compiler version are not ours to fix here.
        .warnings(false)
        .flag_if_supported("-Wno-trigraphs")
        .flag_if_supported("-pthread");
    build.compile("refengine");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        println!("cargo:rustc-link-lib=pthread");
        println!("cargo:rustc-link-lib=m");
    }
}
