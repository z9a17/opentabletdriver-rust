fn main() {
    let root = std::path::Path::new("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", root.display());
    let manifest = std::fs::read_to_string(root).expect("read workspace package manifest");
    let version = manifest.lines().find_map(|line| {
        line.trim().strip_prefix("version = ").map(|value| value.trim_matches('"'))
    }).expect("workspace package version");
    println!("cargo:rustc-env=OTD_RELEASE_VERSION={version}");
}
