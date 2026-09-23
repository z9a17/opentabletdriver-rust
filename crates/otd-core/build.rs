//! Embeds the pinned tablet configuration files in `tablets/` for
//! `tablets::Database::builtin`, as (relative path, contents) pairs in path
//! order.

use std::path::{Path, PathBuf};

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("read tablets directory") {
        let path = entry.expect("read tablets entry").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            files.push(path);
        }
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest directory"))
        .join("tablets");
    println!("cargo:rerun-if-changed=tablets");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();
    let mut code = String::from("pub static FILES: &[(&str, &str)] = &[\n");
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .expect("file under tablets")
            .to_string_lossy()
            .replace('\\', "/");
        code.push_str(&format!(
            "    ({relative:?}, include_str!({:?})),\n",
            file.to_string_lossy()
        ));
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("output directory")).join("tablets.rs");
    std::fs::write(out, code).expect("write tablets.rs");
}
