fn main() {
    // The release integrator changes the root package version once for every
    // platform. Do not report this backend crate's internal version to users.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(manifest).expect("read release manifest");
    let version = text.lines().skip_while(|line| line.trim() != "[package]")
        .skip(1).take_while(|line| !line.trim().starts_with('['))
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "version").then(|| value.trim().trim_matches('"'))
        }).expect("root release version");
    println!("cargo:rustc-env=OTD_RELEASE_VERSION={version}");
}
