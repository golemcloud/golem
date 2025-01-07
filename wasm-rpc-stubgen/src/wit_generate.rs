// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::fs::{OverwriteSafeAction, OverwriteSafeActions, PathExtra};
use crate::log::{log_action, log_action_plan, log_warn_action, LogColorize, LogIndent};
use crate::naming::wit::package_dep_dir_name_from_encoder;
use crate::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, InterfaceStub, StubDefinition,
};
use crate::wit_encode::EncodedWitDir;
use crate::wit_resolve::ResolvedWitDir;
use crate::{cargo, fs, naming};
use anyhow::{anyhow, bail, Context};
use itertools::Itertools;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use wit_encoder::{
    Ident, Interface, Package, PackageItem, PackageName, Params, ResourceFunc, Results,
    StandaloneFunc, Type, TypeDef, World, WorldItem,
};
use wit_parser::PackageId;

pub fn generate_stub_wit_to_target(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Generating",
        format!(
            "stub WIT to {}",
            def.target_wit_path().log_color_highlight()
        ),
    );

    let out = generate_stub_wit_from_stub_def(def)?;
    fs::create_dir_all(def.target_wit_root())?;
    fs::write(def.target_wit_path(), out)?;
    Ok(())
}

pub fn generate_stub_wit_from_stub_def(def: &StubDefinition) -> anyhow::Result<String> {
    Ok(generate_stub_package_from_stub_def(def)?.to_string())
}

pub fn generate_stub_package_from_stub_def(def: &StubDefinition) -> anyhow::Result<Package> {
    let mut package = Package::new(PackageName::new(
        def.source_package_name.namespace.clone(),
        format!("{}-stub", def.source_package_name.name),
        def.source_package_name.version.clone(),
    ));

    let interface_identifier = def.target_interface_name();

    // Stub interface
    {
        let mut stub_interface = Interface::new(interface_identifier.clone());

        // Common used types
        stub_interface.use_type(
            "golem:rpc/types@0.1.0",
            "uri",
            Some(Ident::new("golem-rpc-uri")),
        );
        stub_interface.use_type(
            "wasi:io/poll@0.2.0",
            "pollable",
            Some(Ident::new("wasi-io-pollable")),
        );

        // Used or inlined type defs
        for type_def in def.stub_used_type_defs() {
            stub_interface.use_type(
                type_def.interface_identifier.clone(),
                type_def.type_name.clone(),
                type_def.type_name_alias.clone().map(Ident::from),
            );
        }

        // Async return types
        for interface in def.stub_imported_interfaces() {
            for (function, _) in interface.all_functions() {
                if !function.results.is_empty() {
                    add_async_return_type(def, &mut stub_interface, function, interface)?;
                }
            }
        }

        // Function definitions
        for interface in def.stub_imported_interfaces() {
            let mut stub_functions = Vec::<ResourceFunc>::new();

            // Constructor
            {
                let mut constructor = ResourceFunc::constructor();
                let mut params = match &interface.constructor_params {
                    Some(constructor_params) => constructor_params.to_encoder(def)?,
                    None => Params::empty(),
                };
                params.items_mut().insert(
                    0,
                    (
                        Ident::new("location"),
                        Type::Named(Ident::new("golem-rpc-uri")),
                    ),
                );
                constructor.set_params(params);
                stub_functions.push(constructor);
            }

            // Functions
            for (function, is_static) in interface.all_functions() {
                // Blocking
                {
                    let mut blocking_function = {
                        let function_name = format!("blocking-{}", function.name.clone());
                        if is_static {
                            ResourceFunc::static_(function_name)
                        } else {
                            ResourceFunc::method(function_name)
                        }
                    };
                    blocking_function.set_params(function.params.to_encoder(def)?);
                    if !function.results.is_empty() {
                        blocking_function.set_results(function.results.to_encoder(def)?);
                    }
                    stub_functions.push(blocking_function);
                }

                // Async
                {
                    let mut async_function = {
                        if is_static {
                            ResourceFunc::static_(function.name.clone())
                        } else {
                            ResourceFunc::method(function.name.clone())
                        }
                    };
                    async_function.set_params(function.params.to_encoder(def)?);
                    if !function.results.is_empty() {
                        async_function.set_results(Results::Anon(Type::Named(Ident::new(
                            function.async_result_type(interface),
                        ))));
                    }
                    stub_functions.push(async_function);
                }
            }

            stub_interface.type_def(TypeDef::resource(interface.name.clone(), stub_functions));
        }

        package.interface(stub_interface);
    }

    // Stub world
    {
        let mut stub_world = World::new(def.target_world_name());
        stub_world.named_interface_export(interface_identifier);
        package.world(stub_world);
    }

    Ok(package)
}

fn add_async_return_type(
    def: &StubDefinition,
    stub_interface: &mut Interface,
    function: &FunctionStub,
    owner_interface: &InterfaceStub,
) -> anyhow::Result<()> {
    let context = || {
        anyhow!(
            "Failed to add async return type to stub, stub interface: {}, owner interface: {}, function: {:?}", stub_interface.name(), owner_interface.name, function.name)
    };

    stub_interface.type_def(TypeDef::resource(
        function.async_result_type(owner_interface),
        [
            {
                let mut subscribe = ResourceFunc::method("subscribe");
                subscribe.set_results(Results::Anon(Type::Named(Ident::new("wasi-io-pollable"))));
                subscribe
            },
            {
                let mut get = ResourceFunc::method("get");
                match &function.results {
                    FunctionResultStub::Anon(typ) => {
                        get.set_results(Results::Anon(Type::option(
                            typ.to_encoder(def).with_context(context)?,
                        )));
                    }
                    FunctionResultStub::Named(params) => {
                        Err(anyhow!(
                            "Named parameters are not supported as async stub result, params: {:?}",
                            params
                        ))
                        .with_context(context)?;
                    }
                    FunctionResultStub::SelfType => {
                        Err(anyhow!("Unexpected self return type")).with_context(context)?;
                    }
                }
                get
            },
        ],
    ));

    Ok(())
}

pub fn add_dependencies_to_stub_wit_dir(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Adding",
        format!(
            "WIT dependencies from {} to {}",
            def.config.source_wit_root.log_color_highlight(),
            def.config.target_root.log_color_highlight(),
        ),
    );

    let _indent = LogIndent::new();

    let stub_dep_packages = def.stub_dep_package_ids();

    let target_wit_root = def.target_wit_root();
    let target_deps = target_wit_root.join(naming::wit::DEPS_DIR);

    for (package_id, package, package_sources) in def.packages_with_wit_sources() {
        if !stub_dep_packages.contains(&package_id) || package_id == def.source_package_id {
            log_warn_action(
                "Skipping",
                format!(
                    "package dependency {}",
                    package.name.to_string().log_color_highlight()
                ),
            );
            continue;
        }

        log_action(
            "Copying",
            format!(
                "package dependency {}",
                package.name.to_string().log_color_highlight()
            ),
        );

        let _indent = LogIndent::new();
        for source in &package_sources.files {
            let relative = source.strip_prefix(&def.config.source_wit_root)?;
            let dest = target_wit_root.join(relative);
            log_action(
                "Copying",
                format!(
                    "{} to {}",
                    source.log_color_highlight(),
                    dest.log_color_highlight()
                ),
            );
            fs::copy(source, &dest)?;
        }
    }

    write_embedded_source(
        &target_deps.join("wasm-rpc"),
        "wasm-rpc.wit",
        golem_wasm_rpc::WASM_RPC_WIT,
    )?;

    write_embedded_source(
        &target_deps.join("io"),
        "poll.wit",
        golem_wasm_rpc::WASI_POLL_WIT,
    )?;

    Ok(())
}

fn write_embedded_source(target_dir: &Path, file_name: &str, content: &str) -> anyhow::Result<()> {
    fs::create_dir_all(target_dir)?;

    log_action(
        "Writing",
        format!(
            "{} to {}",
            file_name.log_color_highlight(),
            target_dir.log_color_highlight()
        ),
    );

    fs::write(target_dir.join(file_name), content)?;

    Ok(())
}

#[derive(PartialEq, Eq)]
pub enum UpdateCargoToml {
    Update,
    UpdateIfExists,
    NoUpdate,
}

pub struct AddStubAsDepConfig {
    pub stub_wit_root: PathBuf,
    pub dest_wit_root: PathBuf,
    pub update_cargo_toml: UpdateCargoToml,
}

pub fn add_stub_as_dependency_to_wit_dir(config: AddStubAsDepConfig) -> anyhow::Result<()> {
    log_action(
        "Adding",
        format!(
            "stub dependencies to {} from {}",
            config.dest_wit_root.log_color_highlight(),
            config.stub_wit_root.log_color_highlight()
        ),
    );

    let _indent = LogIndent::new();

    let stub_resolved_wit_root = ResolvedWitDir::new(&config.stub_wit_root)?;
    let stub_package = stub_resolved_wit_root.main_package()?;

    let dest_resolved_wit_root = ResolvedWitDir::new(&config.dest_wit_root)?;

    let mut dest_encoded_wit_root = EncodedWitDir::new(&dest_resolved_wit_root.resolve)?;

    let mut actions = OverwriteSafeActions::new();
    let mut package_names_to_package_path = BTreeMap::<wit_parser::PackageName, PathBuf>::new();

    for (package_name, package_id) in &stub_resolved_wit_root.resolve.package_names {
        let package_sources = stub_resolved_wit_root
            .package_sources
            .get(package_id)
            .ok_or_else(|| anyhow!("Failed to get package sources for {}", package_name))?;

        if *package_id == stub_resolved_wit_root.package_id {
            let package_path = naming::wit::package_wit_dep_dir_from_parser(package_name);

            package_names_to_package_path.insert(package_name.clone(), package_path.clone());

            for source in &package_sources.files {
                actions.add(OverwriteSafeAction::CopyFile {
                    source: source.clone(),
                    target: config
                        .dest_wit_root
                        .join(naming::wit::DEPS_DIR)
                        .join(naming::wit::package_dep_dir_name_from_parser(package_name))
                        .join(PathExtra::new(&source).file_name_to_string()?),
                });
            }
        } else {
            package_names_to_package_path.insert(
                package_name.clone(),
                naming::wit::package_wit_dep_dir_from_package_dir_name(
                    &PathExtra::new(&package_sources.dir).file_name_to_string()?,
                ),
            );

            for source in &package_sources.files {
                actions.add(OverwriteSafeAction::CopyFile {
                    source: source.clone(),
                    target: config
                        .dest_wit_root
                        .join(PathExtra::new(&source).strip_prefix(&config.stub_wit_root)?),
                });
            }
        }
    }

    // Import stub and remove source interfaces
    let dest_main_package_id = dest_resolved_wit_root.package_id;

    let dest_main_package_sources = dest_resolved_wit_root
        .package_sources
        .get(&dest_main_package_id)
        .ok_or_else(|| anyhow!("Failed to get dest main package sources"))?;

    if dest_main_package_sources.files.len() != 1 {
        bail!(
            "Expected exactly one dest main package source, got sources: {}",
            dest_main_package_sources
                .files
                .iter()
                .map(|s| s.log_color_highlight())
                .join(", ")
        );
    }

    let package = dest_encoded_wit_root.package(dest_main_package_id)?;
    // NOTE: wit_encoder "inlines" all transitive imports, so we have to clean up transitive
    //       imports from the source-interface package, given they might have been removed or renamed
    //       in the source, and could create invalid imports.
    remove_world_named_interface_imports(
        package,
        &naming::wit::stub_import_interface_prefix_from_stub_package_name(&stub_package.name)?,
    );
    add_world_named_interface_import(package, &naming::wit::stub_import_name(stub_package)?);
    let content = package.to_string();

    actions.add(OverwriteSafeAction::WriteFile {
        content,
        target: dest_main_package_sources.files[0].clone(),
    });

    // Check overwrites
    // TODO: allow_skip_by_content, decide
    let forbidden_overwrites = actions.run(true, false, log_action_plan)?;
    if !forbidden_overwrites.is_empty() {
        eprintln!("The following files would have been overwritten with new content:");
        for action in forbidden_overwrites {
            eprintln!("  {}", action.target().display());
        }
        eprintln!();
        eprintln!("Use --overwrite to force overwrite.");
    }

    // Optionally update Cargo.toml
    if let Some(target_parent) = config.dest_wit_root.parent() {
        let target_cargo_toml = target_parent.join("Cargo.toml");
        if target_cargo_toml.exists() && target_cargo_toml.is_file() {
            if config.update_cargo_toml != UpdateCargoToml::NoUpdate {
                cargo::is_cargo_component_toml(&target_cargo_toml).context(format!(
                    "The file {target_cargo_toml:?} is not a valid cargo-component project"
                ))?;
                cargo::add_cargo_package_component_deps(
                    &target_cargo_toml,
                    package_names_to_package_path,
                )?;
            }
        } else if config.update_cargo_toml == UpdateCargoToml::Update {
            return Err(anyhow!(
                "Cannot update {:?} file because it does not exist or is not a file",
                target_cargo_toml.log_color_highlight()
            ));
        }
    } else if config.update_cargo_toml == UpdateCargoToml::Update {
        return Err(anyhow!("Cannot update the Cargo.toml file because parent directory of the destination WIT root does not exist."));
    }

    Ok(())
}

trait ToEncoder {
    type EncoderType;
    fn to_encoder(&self, stub_definition: &StubDefinition) -> anyhow::Result<Self::EncoderType>;
}

impl ToEncoder for wit_parser::Type {
    type EncoderType = Type;

    fn to_encoder(&self, def: &StubDefinition) -> anyhow::Result<Self::EncoderType> {
        Ok(match self {
            wit_parser::Type::Bool => Type::Bool,
            wit_parser::Type::U8 => Type::U8,
            wit_parser::Type::U16 => Type::U16,
            wit_parser::Type::U32 => Type::U32,
            wit_parser::Type::U64 => Type::U64,
            wit_parser::Type::S8 => Type::S8,
            wit_parser::Type::S16 => Type::S16,
            wit_parser::Type::S32 => Type::S32,
            wit_parser::Type::S64 => Type::S64,
            wit_parser::Type::F32 => Type::F32,
            wit_parser::Type::F64 => Type::F64,
            wit_parser::Type::Char => Type::Char,
            wit_parser::Type::String => Type::String,
            wit_parser::Type::Id(type_id) => {
                if let Some(type_alias) = def.get_stub_used_type_alias(*type_id) {
                    Type::Named(Ident::new(type_alias.to_string()))
                } else {
                    let typ = def.get_type_def(*type_id)?;

                    let context = || {
                        anyhow!(
                            "Failed to convert wit parser type to encoder type, type: {:?}",
                            typ
                        )
                    };

                    match &typ.kind {
                        wit_parser::TypeDefKind::Option(inner) => Type::option(
                            inner
                                .to_encoder(def)
                                .context("Failed to convert option inner type")
                                .with_context(context)?,
                        ),
                        wit_parser::TypeDefKind::List(inner) => Type::list(
                            inner
                                .to_encoder(def)
                                .context("Failed to convert list inner type")
                                .with_context(context)?,
                        ),
                        wit_parser::TypeDefKind::Tuple(tuple) => Type::tuple(
                            tuple
                                .types
                                .iter()
                                .map(|t| {
                                    t.to_encoder(def).with_context(|| {
                                        anyhow!("Failed to convert tuple elem type: {:?}", t)
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .with_context(context)?,
                        ),
                        wit_parser::TypeDefKind::Result(result) => {
                            match (&result.ok, &result.err) {
                                (Some(ok), Some(err)) => Type::result_both(
                                    ok.to_encoder(def)
                                        .context("Failed to convert result ok type")
                                        .with_context(context)?,
                                    err.to_encoder(def)
                                        .context("Failed to convert result error type")
                                        .with_context(context)?,
                                ),
                                (Some(ok), None) => Type::result_ok(
                                    ok.to_encoder(def)
                                        .context("Failed to convert result ok type (no error case)")
                                        .with_context(context)?,
                                ),
                                (None, Some(err)) => Type::result_err(
                                    err.to_encoder(def)
                                        .context("Failed to convert result error type (no ok case)")
                                        .with_context(context)?,
                                ),
                                (None, None) => Err(anyhow!("Result type has no ok or err types"))
                                    .with_context(context)?,
                            }
                        }
                        wit_parser::TypeDefKind::Handle(handle) => match handle {
                            wit_parser::Handle::Own(type_id) => {
                                wit_parser::Type::Id(*type_id).to_encoder(def)?
                            }
                            wit_parser::Handle::Borrow(type_id) => Type::borrow(
                                wit_parser::Type::Id(*type_id).to_encoder(def)?.to_string(),
                            ),
                        },
                        _ => {
                            let name = typ
                                .name
                                .clone()
                                .ok_or_else(|| anyhow!("Expected name for type: {:?}", typ))
                                .with_context(context)?;
                            Type::Named(Ident::new(name))
                        }
                    }
                }
            }
        })
    }
}

impl ToEncoder for Vec<FunctionParamStub> {
    type EncoderType = Params;

    fn to_encoder(&self, def: &StubDefinition) -> anyhow::Result<Self::EncoderType> {
        Ok(Params::from_iter(
            self.iter()
                .map(|param| {
                    param
                        .typ
                        .to_encoder(def)
                        .with_context(|| {
                            anyhow!(
                                "Failed to convert parameter to encoder type, parameter: {:?}",
                                param
                            )
                        })
                        .map(|typ| (param.name.clone(), typ))
                })
                .collect::<Result<Vec<_>, _>>()
                .with_context(|| anyhow!("Failed to convert params to encoder type: {:?}", self))?,
        ))
    }
}

impl ToEncoder for FunctionResultStub {
    type EncoderType = Results;

    fn to_encoder(&self, def: &StubDefinition) -> anyhow::Result<Self::EncoderType> {
        let context = || {
            anyhow!(
                "Failed to convert function result stub to encoder type: {:?}",
                self
            )
        };
        Ok(match self {
            FunctionResultStub::Anon(typ) => {
                Results::Anon(typ.to_encoder(def).with_context(context)?)
            }
            FunctionResultStub::Named(types) => {
                Results::Named(types.to_encoder(def).with_context(context)?)
            }
            FunctionResultStub::SelfType => {
                Err(anyhow!("Unexpected self type")).with_context(context)?
            }
        })
    }
}

fn add_world_named_interface_import(package: &mut Package, import_name: &str) {
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

fn remove_world_named_interface_imports(package: &mut Package, import_prefix: &str) {
    for world_item in package.items_mut() {
        if let PackageItem::World(world) = world_item {
            world.items_mut().retain(|item| {
                if let WorldItem::NamedInterfaceImport(import) = item {
                    !import.name().raw_name().starts_with(import_prefix)
                } else {
                    true
                }
            })
        }
    }
}

pub fn extract_main_interface_as_wit_dep(wit_dir: &Path) -> anyhow::Result<()> {
    log_action(
        "Extracting",
        format!(
            "interface package from main package in wit directory {}",
            wit_dir.log_color_highlight()
        ),
    );

    let resolved_wit_dir = ResolvedWitDir::new(wit_dir)?;
    let main_package_id = resolved_wit_dir.package_id;
    let mut encoded_wit_dir = EncodedWitDir::new(&resolved_wit_dir.resolve)?;

    let resolved_wit_dir = ResolvedWitDir::new(wit_dir)?;

    let (main_package, interface_package) =
        extract_main_interface_package(main_package_id, &mut encoded_wit_dir)?;
    let sources = resolved_wit_dir
        .package_sources
        .get(&resolved_wit_dir.package_id)
        .ok_or_else(|| {
            anyhow!(
                "Failed to get sources for main package, wit dir: {}",
                wit_dir.log_color_highlight()
            )
        })?;

    if sources.files.len() != 1 {
        bail!(
            "Expected exactly one source for main package, wit dir: {}",
            wit_dir.log_color_highlight()
        );
    }

    let _indent = LogIndent::new();

    let interface_package_path = wit_dir
        .join(naming::wit::DEPS_DIR)
        .join(package_dep_dir_name_from_encoder(interface_package.name()))
        .join(naming::wit::INTERFACE_WIT_FILE_NAME);
    log_action(
        "Writing",
        format!(
            "interface package to {}",
            interface_package_path.log_color_highlight()
        ),
    );
    fs::write_str(&interface_package_path, interface_package.to_string())?;

    let main_package_path = &sources.files[0];
    log_action(
        "Writing",
        format!(
            "main package to {}",
            main_package_path.log_color_highlight()
        ),
    );
    fs::write_str(main_package_path, main_package.to_string())?;

    Ok(())
}

// TODO: handle world include
// TODO: handle world use
// TODO: maybe transform inline interfaces and functions into included world?
fn extract_main_interface_package(
    main_package_id: PackageId,
    encoded_wit_dir: &mut EncodedWitDir,
) -> anyhow::Result<(Package, Package)> {
    let package = encoded_wit_dir.package(main_package_id)?;

    let mut interface_package = package.clone();
    interface_package.set_name(naming::wit::interface_encoder_package_name(package.name()));

    let interface_prefix = format!(
        "{}:{}/",
        package.name().namespace(),
        interface_package.name().name()
    );
    let interface_suffix = package
        .name()
        .version()
        .map(|version| format!("@{}", version))
        .unwrap_or_default();

    let mut exported_interface_identifiers = HashSet::<Ident>::new();
    let mut inline_interface_exports = BTreeMap::<Ident, Vec<Interface>>::new();
    let mut inline_function_exports = BTreeMap::<Ident, Vec<StandaloneFunc>>::new();
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            let world_name = world.name().clone();

            world.items_mut().retain(|world_item| match world_item {
                // Remove and collect inline interface exports
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
                // Remove and collect inline function exports
                WorldItem::FunctionExport(function) => {
                    inline_function_exports
                        .entry(world_name.clone())
                        .or_default()
                        .push(function.clone());
                    false
                }
                // Collect named interface export identifiers
                WorldItem::NamedInterfaceExport(interface) => {
                    exported_interface_identifiers.insert(interface.name().clone());
                    true
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
                    interface_prefix,
                    naming::wit::interface_package_world_inline_functions_interface_name(
                        &world_name
                    ),
                    interface_suffix
                ));
            }
        }
    }

    package.items_mut().retain(|item| match item {
        // Drop exported interfaces from original package
        PackageItem::Interface(interface) => {
            !exported_interface_identifiers.contains(interface.name())
        }
        PackageItem::World(_) => true,
    });

    interface_package.items_mut().retain(|item| match item {
        // Drop non-exported interfaces from interface package
        PackageItem::Interface(interface) => {
            exported_interface_identifiers.contains(interface.name())
        }
        // Drop all worlds from interface package
        PackageItem::World(_) => false,
    });

    // Rename named self export and imports to use the extracted interface names
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            for world_item in world.items_mut() {
                if let WorldItem::NamedInterfaceImport(import) = world_item {
                    if !import.name().raw_name().contains("/") {
                        import.set_name(format!(
                            "{}{}{}",
                            interface_prefix,
                            import.name(),
                            interface_suffix
                        ));
                    }
                } else if let WorldItem::NamedInterfaceExport(export) = world_item {
                    if !export.name().raw_name().contains("/") {
                        export.set_name(format!(
                            "{}{}{}",
                            interface_prefix,
                            export.name(),
                            interface_suffix
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
