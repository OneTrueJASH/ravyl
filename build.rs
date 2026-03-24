use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let refengine_dir = PathBuf::from(&manifest_dir).join("external/refengine");

    println!("cargo:rustc-link-search=native={}", refengine_dir.display());
    println!("cargo:rustc-link-lib=dylib=refengine");

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", refengine_dir.display());
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../external/refengine");
    }

    println!(
        "cargo:rerun-if-changed={}",
        refengine_dir.join("librefengine.dylib").display()
    );
}
