fn main() {
    #[cfg(not(feature = "test-fixture"))]
    let key = include!("../../../crates/rz-services/src/update_key.in");
    #[cfg(feature = "test-fixture")]
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32])
        .verifying_key()
        .to_bytes();
    if let Err(error) = rz_app::update_runtime::helper_main(std::env::args_os().skip(1), &key) {
        eprintln!("RustZIP updater: {error}");
        std::process::exit(1);
    }
}
