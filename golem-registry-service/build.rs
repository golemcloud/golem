use std::{env, fs, path::PathBuf};

#[path = "src/builtin_manifest.rs"]
mod builtin_manifest;

fn main() -> anyhow::Result<()> {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .unwrap()
        .to_path_buf();
    let manifest = root.join("builtin-artifacts.release.json");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    println!("cargo:rustc-check-cfg=cfg(golem_builtin_release)");
    println!("cargo:rerun-if-env-changed=GOLEM_BUILTIN_RELEASE");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/builtin_manifest.rs");
    println!(
        "cargo:rerun-if-changed={}",
        root.join("builtin-artifacts.json").display()
    );
    let contents = if env::var_os("GOLEM_BUILTIN_RELEASE").is_some() {
        anyhow::ensure!(
            !cfg!(feature = "embed-builtin-artifacts"),
            "Published release binaries must download built-ins; do not enable embed-builtin-artifacts with GOLEM_BUILTIN_RELEASE"
        );
        println!("cargo:rustc-cfg=golem_builtin_release");
        println!("cargo:rerun-if-changed={}", manifest.display());
        let contents = fs::read_to_string(&manifest)?;
        builtin_manifest::parse_manifest(&contents)?;
        contents.into_bytes()
    } else {
        Vec::new()
    };
    fs::write(out.join("builtin-artifacts.release.json"), contents)?;
    #[cfg(feature = "embed-builtin-artifacts")]
    {
        use std::io::Write;
        let mut generated = String::from("[\n");
        for (id, entry) in builtin_manifest::inventory()? {
            let path = root.join(entry.path);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = fs::read(&path).map_err(|error| {
                anyhow::anyhow!(
                    "Cannot embed '{}': {error}; run cargo make ensure-builtin-artifacts",
                    path.display()
                )
            })?;
            anyhow::ensure!(
                !bytes.is_empty(),
                "Cannot embed empty built-in artifact '{id}'"
            );
            let compressed = out.join(format!("{id}.wasm.zst"));
            let mut encoder = zstd::stream::write::Encoder::new(fs::File::create(&compressed)?, 3)?;
            encoder.write_all(&bytes)?;
            encoder.finish()?;
            generated.push_str(&format!(
                "({id:?}, include_bytes!({compressed:?}).as_slice()),\n"
            ));
        }
        generated.push(']');
        fs::write(out.join("builtin-artifacts.rs"), generated)?;
    }
    Ok(())
}
