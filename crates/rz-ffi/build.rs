use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=VCPKG_INSTALLATION_ROOT");
    let target = env::var("TARGET").unwrap_or_default();
    if target == "x86_64-pc-windows-msvc" {
        let Some(root) = env::var_os("VCPKG_INSTALLATION_ROOT") else {
            println!("cargo:warning=VCPKG_INSTALLATION_ROOT is required for native libarchive");
            return;
        };
        let lib = PathBuf::from(root).join("installed/x64-windows/lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
        // The separate name prevents another crate's static lzma.lib from shadowing
        // the vcpkg import library required by libarchive's __imp_lzma symbols.
        println!("cargo:rustc-link-lib=dylib=rustzip_lzma_import");
    }
}
