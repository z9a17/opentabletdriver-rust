use std::{env, error::Error, fs, path::PathBuf, process::Command};

fn resource_compiler() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = env::var_os("RC") {
        return Ok(path.into());
    }
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join("rc.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let sdk = match env::var_os("WindowsSdkDir") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("ProgramFiles(x86)").ok_or(
            "cannot find Windows SDK; set RC to the resource compiler's absolute path",
        )?)
        .join("Windows Kits/10"),
    };
    let mut versions = fs::read_dir(sdk.join("bin"))?
        .collect::<Result<Vec<_>, _>>()?;
    versions.sort_by_key(|entry| entry.file_name());
    versions.reverse();
    versions
        .into_iter()
        .map(|entry| entry.path().join("x64/rc.exe"))
        .find(|path| path.is_file())
        .ok_or_else(|| "Windows SDK rc.exe not found; set RC to its absolute path".into())
}

fn main() -> Result<(), Box<dyn Error>> {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return Ok(());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = root.join("resources/windows.manifest");
    let icon = root.join("resources/opentabletdriver.ico");
    for path in [&manifest, &icon] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for name in ["RC", "WINDRES", "WindowsSdkDir", "ProgramFiles(x86)"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let script = out.join("application.rc");
    let icon_path = icon.display().to_string().replace('\\', "/");
    let mut resource = format!("1 ICON \"{icon_path}\"\n");
    match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => {
            fs::write(&script, resource)?;
            let compiled = out.join("application.res");
            let status = Command::new(resource_compiler()?)
                .args(["/nologo", "/fo"])
                .arg(&compiled)
                .arg(&script)
                .status()?;
            if !status.success() {
                return Err("Windows application icon resource compilation failed".into());
            }
            println!("cargo:rustc-link-arg-bins={}", compiled.display());
            println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
        }
        Ok("gnu") => {
            let manifest_path = manifest.display().to_string().replace('\\', "/");
            resource.push_str(&format!("1 24 \"{manifest_path}\"\n"));
            fs::write(&script, resource)?;
            let compiled = out.join("application.o");
            let windres = env::var("WINDRES").unwrap_or_else(|_| {
                if cfg!(windows) {
                    "windres".into()
                } else {
                    "x86_64-w64-mingw32-windres".into()
                }
            });
            let status = Command::new(windres)
                .args(["-O", "coff", "-i"])
                .arg(&script)
                .arg("-o")
                .arg(&compiled)
                .status()?;
            if !status.success() {
                return Err("Windows application icon/manifest resource compilation failed".into());
            }
            println!("cargo:rustc-link-arg-bins={}", compiled.display());
        }
        _ => return Err("unsupported Windows resource toolchain".into()),
    }
    Ok(())
}
