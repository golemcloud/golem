use anyhow::{anyhow, bail, Context};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use wit_parser::{PackageId, Resolve, UnresolvedPackageGroup};

pub struct ResolvedWitDir {
    pub resolve: Resolve,
    pub package_id: PackageId,
    pub sources: IndexMap<PackageId, Vec<PathBuf>>,
}

pub fn resolve_wit_dir(path: &Path) -> anyhow::Result<ResolvedWitDir> {
    // TODO: Can be removed once we fixed all docs and examples
    std::env::set_var("WIT_REQUIRE_F32_F64", "0");

    let mut resolve = Resolve::new();

    let (package_id, sources) = resolve
        .push_dir(path)
        .with_context(|| anyhow!("failed to resolve wit dir: {}", path.to_string_lossy()))?;

    let sources = partition_sources_by_package_ids(path, &resolve, package_id, sources)?;

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
// and also partially parsing again deps.
fn partition_sources_by_package_ids(
    path: &Path,
    resolve: &Resolve,
    root_package_id: PackageId,
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

    let mut partitioned_sources = IndexMap::<PackageId, Vec<PathBuf>>::new();
    let mut dep_path_to_package_id = IndexMap::<PathBuf, PackageId>::new();

    for source in sources {
        let relative_source = source.strip_prefix(path)?;

        let segments = relative_source.iter().collect::<Vec<_>>();

        let package_id = match segments.len() {
            1 => root_package_id,
            2 => {
                bail!(
                    "Single file with packages not supported, source: {}",
                    source.to_string_lossy()
                );
            }
            3 => {
                let dep_package_path = source.parent().ok_or_else(|| {
                    anyhow!(
                        "Failed to get source parent, source: {}",
                        source.to_string_lossy()
                    )
                })?;

                match dep_path_to_package_id.get(dep_package_path) {
                    Some(package_id) => *package_id,
                    None => {
                        let package_id = *resolve
                            .package_names
                            .get(
                                &UnresolvedPackageGroup::parse_dir(dep_package_path)?
                                    .main
                                    .name,
                            )
                            .ok_or_else(|| {
                                anyhow!(
                                    "Failed to get package id for source: {}",
                                    source.to_string_lossy(),
                                )
                            })?;

                        dep_path_to_package_id.insert(dep_package_path.to_path_buf(), package_id);

                        package_id
                    }
                }
            }
            _ => {
                bail!(
                    "Unexpected source path, source: {}",
                    source.to_string_lossy()
                );
            }
        };

        partitioned_sources
            .entry(package_id)
            .and_modify(|sources| sources.push(source.to_path_buf()))
            .or_insert_with(|| vec![source.to_path_buf()]);
    }

    Ok(partitioned_sources)
}
