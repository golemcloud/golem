use anyhow::anyhow;
use indexmap::IndexMap;
use regex::Regex;
use wit_encoder::{packages_from_parsed, Package, PackageItem, WorldItem};

// TODO: use wit transform everywhere
pub fn import_remover(
    package_name: &wit_parser::PackageName,
) -> impl Fn(String) -> anyhow::Result<String> {
    let pattern_import_stub_package_name = Regex::new(
        format!(
            r"import\s+{}(/[^;]*)?;",
            regex::escape(&package_name.to_string())
        )
        .as_str(),
    )
    .unwrap_or_else(|err| panic!("Failed to compile package import regex: {}", err));

    move |src: String| -> anyhow::Result<String> {
        Ok(pattern_import_stub_package_name
            .replace_all(&src, "")
            .to_string())
    }
}

pub struct WitTransformer {
    encoded_packages_by_parser_id: IndexMap<wit_parser::PackageId, Package>,
}

impl WitTransformer {
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

    pub fn render_package(&mut self, package_id: wit_parser::PackageId) -> anyhow::Result<String> {
        Ok(format!("{}", self.encoded_package(package_id)?))
    }

    fn encoded_package(
        &mut self,
        package_id: wit_parser::PackageId,
    ) -> anyhow::Result<&mut Package> {
        self.encoded_packages_by_parser_id
            .get_mut(&package_id)
            .ok_or_else(|| anyhow!("Failed to get encoded package by id: {:?}", package_id))
    }

    pub fn remove_imports_from_package_all_worlds(
        &mut self,
        package_id: wit_parser::PackageId,
        import_prefix: &str,
    ) -> anyhow::Result<()> {
        let package = self.encoded_package(package_id)?;
        for world_item in package.items_mut() {
            match world_item {
                PackageItem::Interface(_) => {}
                PackageItem::World(world) => {
                    world.items_mut().retain(|item| {
                        if let WorldItem::NamedInterfaceImport(import) = &item {
                            !import.name().raw_name().starts_with(import_prefix)
                        } else {
                            true
                        }
                    });
                }
            }
        }

        Ok(())
    }

    pub fn add_import_to_all_world(
        &mut self,
        package_id: wit_parser::PackageId,
        import_name: &str,
    ) -> anyhow::Result<()> {
        let package = self.encoded_package(package_id)?;
        for world_item in package.items_mut() {
            match world_item {
                PackageItem::Interface(_) => {}
                PackageItem::World(world) => {
                    let is_already_imported = world.items_mut().iter().any(|item| {
                        if let WorldItem::NamedInterfaceImport(import) = item {
                            import.name().raw_name() == import_name
                        } else {
                            false
                        }
                    });
                    if !is_already_imported {
                        world.named_interface_import(import_name.to_string());
                    }
                }
            }
        }

        Ok(())
    }
}
