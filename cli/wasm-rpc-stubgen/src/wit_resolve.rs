use crate::fs::strip_path_prefix;
use anyhow::{anyhow, bail, Context};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use wit_parser::{Package, PackageId, PackageName, Resolve, UnresolvedPackageGroup};

pub struct ResolvedWitDir {
    pub path: PathBuf,
    pub resolve: Resolve,
    pub package_id: PackageId,
    pub sources: IndexMap<PackageId, (PathBuf, Vec<PathBuf>)>,
}

impl ResolvedWitDir {
    pub fn new(path: &Path) -> anyhow::Result<ResolvedWitDir> {
        resolve_wit_dir(path)
    }

    pub fn package(&self, package_id: PackageId) -> anyhow::Result<&Package> {
        self.resolve.packages.get(package_id).with_context(|| {
            anyhow!(
                "Failed to get package by id: {:?}, wit dir: {}",
                package_id,
                self.path.to_string_lossy()
            )
        })
    }

    pub fn package_id_by_encoder_name(
        &self,
        package_name: &wit_encoder::PackageName,
    ) -> Option<PackageId> {
        let package_name = PackageName {
            namespace: package_name.namespace().to_string(),
            name: package_name.name().to_string(),
            version: package_name.version().cloned(),
        };

        self.resolve.package_names.get(&package_name).cloned()
    }

    pub fn main_package(&self) -> anyhow::Result<&Package> {
        self.package(self.package_id)
    }
}

fn resolve_wit_dir(path: &Path) -> anyhow::Result<ResolvedWitDir> {
    // TODO: Can be removed once we fixed all docs and examples
    std::env::set_var("WIT_REQUIRE_F32_F64", "0");

    let mut resolve = Resolve::new();

    let (package_id, sources) = resolve
        .push_dir(path)
        .with_context(|| anyhow!("Failed to resolve wit dir: {}", path.to_string_lossy()))?;

    let sources = partition_sources_by_package_ids(path, &resolve, package_id, sources)?;

    Ok(ResolvedWitDir {
        path: path.to_path_buf(),
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
// Instead of that, we partition the returned sources by following the accepted file and directory structure.
//
// Unfortunately we still have some duplication of performed operations: during partitioning we partially parse
// the dependencies again - as UnresolvedPackageGroups - but similar partial "peeks" into deps already happened
// in stubgen steps before. This way we try to pull and concentrate them here, so they only happen when creating
// a new ResolvedWitDir, and parsing should only happen twice: while using Resolve::push_dir above, and here.
fn partition_sources_by_package_ids(
    path: &Path,
    resolve: &Resolve,
    root_package_id: PackageId,
    sources: Vec<PathBuf>,
) -> anyhow::Result<IndexMap<PackageId, (PathBuf, Vec<PathBuf>)>> {
    // Based on Resolve::push_dir ():
    //
    // The deps folder may contain:
    //   $path/ deps/ my-package/*.wit: a directory that may contain multiple WIT files
    //   $path/ deps/ my-package. wit: a single-file WIT package
    //   $path/ deps/ my-package.{wasm,wat}: a wasm-encoded WIT package either in text or binary format
    //
    // Disabling "wasm" and "wat" sources could be done by disabling default features, but they are also required through other dependencies,
    // so we filter out currently not supported path patterns here, including the single file format (for now).

    let mut partitioned_sources = IndexMap::<PackageId, (PathBuf, Vec<PathBuf>)>::new();
    let mut dep_package_path_to_package_id = IndexMap::<PathBuf, PackageId>::new();

    for source in sources {
        let relative_source = strip_path_prefix(path, &source)?;

        let segments = relative_source.iter().collect::<Vec<_>>();

        let (package_path, package_id) = match segments.len() {
            1 => (path, root_package_id),
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

                match dep_package_path_to_package_id.get(dep_package_path) {
                    Some(package_id) => (dep_package_path, *package_id),
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

                        dep_package_path_to_package_id
                            .insert(dep_package_path.to_path_buf(), package_id);

                        (dep_package_path, package_id)
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
            .and_modify(|(_, sources)| sources.push(source.to_path_buf()))
            .or_insert_with(|| (package_path.to_path_buf(), vec![source.to_path_buf()]));
    }

    Ok(partitioned_sources)
}
