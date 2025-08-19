use anyhow::anyhow;
use indexmap::IndexMap;
use wit_encoder::{packages_from_parsed, Package};

pub struct EncodedWitDir {
    encoded_packages_by_parser_id: IndexMap<wit_parser::PackageId, Package>,
}

impl EncodedWitDir {
    pub fn new(resolve: &wit_parser::Resolve) -> anyhow::Result<Self> {
        let mut encoded_packages_by_parser_id = IndexMap::<wit_parser::PackageId, Package>::new();

        for package in packages_from_parsed(resolve) {
            let package_name = package.name();
            let package_name = wit_parser::PackageName {
                namespace: package_name.namespace().to_string(),
                name: package_name.name().to_string(),
                version: package_name.version().cloned(),
            };

            let package_id = resolve
                .package_names
                .get(&package_name)
                .cloned()
                .ok_or_else(|| anyhow!("Failed to get package by name: {}", package.name()))?;
            encoded_packages_by_parser_id.insert(package_id, package);
        }

        Ok(Self {
            encoded_packages_by_parser_id,
        })
    }

    pub fn package(&mut self, package_id: wit_parser::PackageId) -> anyhow::Result<&mut Package> {
        self.encoded_packages_by_parser_id
            .get_mut(&package_id)
            .ok_or_else(|| anyhow!("Failed to get encoded package by id: {:?}", package_id))
    }
}
