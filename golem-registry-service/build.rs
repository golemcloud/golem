use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = root.join("builtin-artifacts.release.json");
    println!("cargo:rustc-check-cfg=cfg(golem_builtin_release)");
    println!("cargo:rerun-if-env-changed=GOLEM_BUILTIN_RELEASE");
    println!("cargo:rerun-if-changed=build.rs");
    let contents = if env::var_os("GOLEM_BUILTIN_RELEASE").is_some() {
        println!("cargo:rustc-cfg=golem_builtin_release");
        println!("cargo:rerun-if-changed={}", manifest.display());
        let contents = fs::read(&manifest)?;
        if contents.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "builtin-artifacts.release.json must not be empty",
            ));
        }
        contents
    } else {
        Vec::new()
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("builtin-artifacts.release.json"),
        contents,
    )
}
