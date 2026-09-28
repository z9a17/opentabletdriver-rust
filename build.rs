fn main() {
    let manifest =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/windows.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => {
            // Embed through link.exe so the build needs no resource compiler.
            println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
            println!(
                "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
                manifest.display()
            );
        }
        // MinGW's linker has no manifest option: compile the same manifest as
        // the application manifest resource (ID 1, RT_MANIFEST).
        Ok("gnu") if windows => {
            println!("cargo:rerun-if-env-changed=WINDRES");
            let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
            let script = out.join("manifest.rc");
            let object = out.join("manifest.o");
            let path = manifest.display().to_string().replace('\\', "/");
            std::fs::write(&script, format!("1 24 \"{path}\"\n")).unwrap();
            let windres = std::env::var("WINDRES").unwrap_or_else(|_| {
                if cfg!(windows) {
                    "windres".into()
                } else {
                    "x86_64-w64-mingw32-windres".into()
                }
            });
            let compiled = std::process::Command::new(&windres)
                .arg("-O")
                .arg("coff")
                .arg("-i")
                .arg(&script)
                .arg("-o")
                .arg(&object)
                .status();
            match compiled {
                Ok(status) if status.success() => {
                    println!("cargo:rustc-link-arg-bins={}", object.display());
                }
                _ => println!(
                    "cargo:warning={windres} failed; the executables have no application manifest"
                ),
            }
        }
        _ => {}
    }
}
