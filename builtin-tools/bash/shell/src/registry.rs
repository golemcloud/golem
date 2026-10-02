use crate::manifest::Manifest;
pub(crate) fn build() -> std::collections::HashMap<String, Manifest> {
    crate::tools::coreutils::manifests()
        .into_iter()
        .chain(crate::tools::texttools::manifests())
        .chain(crate::tools::find::manifests())
        .chain(crate::tools::stat::manifests())
        .chain(crate::tools::install::manifests())
        .chain(crate::tools::which::manifests())
        .chain(crate::tools::man::manifests())
        .chain(crate::tools::xargs::manifests())
        .chain(crate::tools::sh::manifests())
        .chain(crate::tools::timeout::manifests())
        .map(|manifest| (manifest.name.clone(), manifest))
        .collect()
}
