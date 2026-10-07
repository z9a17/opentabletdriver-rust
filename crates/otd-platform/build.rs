fn main() {
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest=root.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}",manifest.display());
    let text=std::fs::read_to_string(manifest).expect("read root release manifest");
    let version=text.lines().skip_while(|line|line.trim()!="[package]").skip(1).take_while(|line|!line.trim().starts_with('['))
        .find_map(|line|line.split_once('=').filter(|(key,_)|key.trim()=="version").map(|(_,value)|value.trim().trim_matches('"'))).expect("root release version");
    println!("cargo:rustc-env=OTD_RELEASE_VERSION={version}");
    let date=std::process::Command::new("git").current_dir(root).args(["show","-s","--format=%cI","HEAD"]).output().expect("read source commit date");
    assert!(date.status.success(),"cannot read build source date");
    println!("cargo:rustc-env=OTD_BUILD_DATE={}",String::from_utf8(date.stdout).expect("source date UTF-8").trim());
}
