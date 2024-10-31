use crate::naming;
use crate::wit_encode::EncodedWitDir;
use crate::wit_resolve::ResolvedWitDir;
use regex::Regex;
use std::collections::BTreeMap;
use wit_encoder::{Ident, Interface, Package, PackageItem, StandaloneFunc, WorldItem};

// TODO: use wit encoder based one
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

// TODO: make the import name matcher more precise
pub fn remove_world_named_interface_imports(package: &mut Package, import_prefix: &str) {
    for world_item in package.items_mut() {
        if let PackageItem::World(world) = world_item {
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

pub fn add_import_to_all_world(package: &mut Package, import_name: &str) {
    for world_item in package.items_mut() {
        if let PackageItem::World(world) = world_item {
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

// TODO: handle world include
// TODO: handle world use
// TODO: maybe transform inline interfaces and functions into included world?
pub fn extract_main_interface_package(
    resolve: &ResolvedWitDir,
) -> anyhow::Result<(Package, Package)> {
    let mut encoded_wit_dir = EncodedWitDir::new(&resolve.resolve)?;

    let package = encoded_wit_dir.package(resolve.package_id)?;

    let mut interface_package = package.clone();
    interface_package.set_name(naming::wit::interface_package_name(package.name()));

    let interface_export_prefix = format!(
        "{}:{}/",
        package.name().namespace(),
        interface_package.name().name()
    );
    let interface_export_suffix = package
        .name()
        .version()
        .map(|version| format!("@{}", version))
        .unwrap_or_default();

    // Drop all interfaces from original package
    package.items_mut().retain(|item| match item {
        PackageItem::Interface(_) => false,
        PackageItem::World(_) => true,
    });
    // Drop all worlds from interface package
    interface_package.items_mut().retain(|item| match item {
        PackageItem::Interface(_) => true,
        PackageItem::World(_) => false,
    });

    let mut inline_interface_exports = BTreeMap::<Ident, Vec<Interface>>::new();
    let mut inline_function_exports = BTreeMap::<Ident, Vec<StandaloneFunc>>::new();
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            let world_name = world.name().clone();

            // Remove and collect inline exports
            world.items_mut().retain(|world_item| match world_item {
                WorldItem::InlineInterfaceExport(interface) => {
                    let mut interface = interface.clone();
                    interface.set_name(naming::wit::interface_package_world_inline_interface_name(
                        &world_name,
                        interface.name(),
                    ));

                    inline_interface_exports
                        .entry(world_name.clone())
                        .or_default()
                        .push(interface.clone());
                    false
                }
                WorldItem::FunctionExport(function) => {
                    inline_function_exports
                        .entry(world_name.clone())
                        .or_default()
                        .push(function.clone());
                    false
                }
                _ => true,
            });

            // Insert named imports for extracted inline interfaces
            if let Some(interfaces) = inline_interface_exports.get(&world_name) {
                for interface in interfaces {
                    world.named_interface_export(interface.name().clone());
                }
            }

            // Insert named import for extracted inline functions
            if inline_function_exports.contains_key(&world_name) {
                world.named_interface_export(format!(
                    "{}{}{}",
                    interface_export_prefix,
                    naming::wit::interface_package_world_inline_functions_interface_name(
                        &world_name
                    ),
                    interface_export_suffix
                ));
            }
        }
    }

    // Rename named self imports to use the extracted interface names
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            for world_item in world.items_mut() {
                if let WorldItem::NamedInterfaceExport(export) = world_item {
                    if !export.name().raw_name().contains("/") {
                        export.set_name(format!(
                            "{}{}{}",
                            interface_export_prefix,
                            export.name(),
                            interface_export_suffix
                        ));
                    }
                }
            }
        }
    }

    // Add inlined exported interfaces to the interface package
    for (_, interfaces) in inline_interface_exports {
        for interface in interfaces {
            interface_package.interface(interface);
        }
    }

    // Add interface for inlined functions to the interface package
    for (world_name, functions) in inline_function_exports {
        let mut interface = Interface::new(
            naming::wit::interface_package_world_inline_functions_interface_name(&world_name),
        );

        for function in functions {
            interface.function(function);
        }

        interface_package.interface(interface);
    }

    Ok((package.clone(), interface_package))
}

// TODO: delete these
#[cfg(test)]
mod test {
    use test_r::test;

    use crate::wit_resolve::ResolvedWitDir;
    use crate::wit_transform::extract_main_interface_package;
    use std::path::Path;

    #[test]
    fn test_extract_playground() {
        let resolved_wit_dir =
            ResolvedWitDir::new(Path::new("test-data/many-ways-to-export")).unwrap();
        let (package, interface_package) =
            extract_main_interface_package(&resolved_wit_dir).unwrap();

        println!("{}", package);
        println!("\n---\n");
        println!("{}", interface_package);
    }
}
