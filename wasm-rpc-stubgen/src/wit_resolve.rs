use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use wit_parser::{PackageId, Resolve};

pub struct ResolvedWitDir {
    pub resolve: Resolve,
    pub package_id: PackageId,
    pub sources: IndexMap<PackageId, Vec<PathBuf>>,
}

pub fn resolve_wit_dir(path: &Path) -> anyhow::Result<ResolvedWitDir> {
    let mut resolve = Resolve::new();

    let (package_id, sources) = resolve
        .push_dir(path)
        .with_context(|| anyhow!("failed to resolve wit dir: {}", path.to_string_lossy()))?;

    let sources = partition_sources_by_package_ids(&resolve, sources)?;

    Ok(ResolvedWitDir {
        resolve,
        package_id,
        sources,
    })
}

// Currently Resolve::push_dir does return the source that were used during resolution,
// but they are not partitioned by packages, and the source info is not kept inside Resolve
// (it is lost during resolution).
//
// To solve this (until we can get a better API upstream) we could extract and replicate the logic used
// there, but that would require many code duplication, as many functions and types are not public.
//
// Instead of that, we partition the returned sources following the accepted file and directory structure,
// and given it is quite strict, we can do so without ambiguities.
fn partition_sources_by_package_ids(
    resolve: &Resolve,
    sources: Vec<PathBuf>,
) -> anyhow::Result<IndexMap<PackageId, Vec<PathBuf>>> {
    // Based on Resolve::push_dir ():
    //
    // The deps folder may contain:
    //   $path/ deps/ my-package/*.wit: a directory that may contain multiple WIT files
    //   $path/ deps/ my-package. wit: a single-file WIT package
    //   $path/ deps/ my-package.{wasm,wat}: a wasm-encoded WIT package either in text or binary format
    //
    // Disabling "wasm" and "wat" sources could be done by disabling default features, but they are also required through other dependencies,
    // so we filter out currently not supported path patterns here, including the single file format (for now).

    for source in sources {
        println!("{}", source.to_string_lossy())
    }

    todo!();
}
