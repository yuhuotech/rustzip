use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=RZ_STATS_URL");
    let stats_url = env::var("RZ_STATS_URL")
        .ok()
        .filter(|url| url.starts_with("https://") && !url.chars().any(char::is_control))
        .unwrap_or_default();
    println!("cargo:rustc-env=RZ_STATS_URL={stats_url}");
}
