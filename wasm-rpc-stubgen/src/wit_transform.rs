use regex::Regex;
use wit_encoder::{Package, PackageItem, WorldItem};

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

pub fn remove_imports_from_package_all_worlds(package: &mut Package, import_prefix: &str) {
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
}

pub fn add_import_to_all_world(package: &mut Package, import_name: &str) {
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
}
