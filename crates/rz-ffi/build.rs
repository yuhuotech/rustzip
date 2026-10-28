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
        let triplet =
            env::var("VCPKG_DEFAULT_TRIPLET").unwrap_or_else(|_| "x64-windows-static-md".into());
        println!("cargo:rerun-if-env-changed=VCPKG_DEFAULT_TRIPLET");
        let lib = PathBuf::from(root)
            .join("installed")
            .join(triplet)
            .join("lib");
        println!("cargo:rustc-link-search=native={}", lib.display());
    }
}
