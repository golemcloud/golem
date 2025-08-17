// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::fs;
use crate::fs::{OverwriteSafeAction, OverwriteSafeActions, PathExtra};
use crate::log::{log_action, log_action_plan, log_warn_action, LogColorize, LogIndent};
use crate::wasm_rpc_stubgen::naming::wit::{
    package_dep_dir_name_from_encoder, package_dep_dir_name_from_parser,
};
use crate::wasm_rpc_stubgen::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, StubDefinition,
};
use crate::wasm_rpc_stubgen::wit_encode::EncodedWitDir;
use crate::wasm_rpc_stubgen::wit_resolve::{ResolvedWitDir, WitDepsResolver};
use crate::wasm_rpc_stubgen::{cargo, naming, GOLEM_RPC_WIT, WASI_CLOCKS_WIT, WASI_IO_WIT};
use crate::wasm_rpc_stubgen::{GOLEM_RPC_WIT_VERSION, WASI_WIT_VERSION};
use anyhow::{anyhow, bail, Context};
use itertools::Itertools;
use semver::Version;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use wit_component::WitPrinter;
use wit_encoder::{
    packages_from_parsed, Ident, Interface, InterfaceItem, Package, PackageItem, Params,
    ResourceFunc, ResourceFuncKind, StandaloneFunc, Type, TypeDef, TypeDefKind, Use, World,
    WorldItem,
};
use wit_parser::{PackageId, PackageName, Resolve, UnresolvedPackageGroup};

use super::stub::StubbedEntity;

pub fn generate_client_wit_to_target(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Generating",
        format!(
            "client WIT to {}",
            def.client_wit_path().log_color_highlight()
        ),
    );

    let out = generate_client_wit_from_stub_def(def)?;
    fs::create_dir_all(def.client_wit_root())?;
    fs::write(def.client_wit_path(), out)?;
    Ok(())
}

pub fn generate_client_wit_from_stub_def(def: &StubDefinition) -> anyhow::Result<String> {
    Ok(generate_client_package_from_stub_def(def)?.to_string())
}

pub fn generate_client_package_from_stub_def(def: &StubDefinition) -> anyhow::Result<Package> {
    let mut package = Package::new(def.client_encoder_package_name());

    let interface_identifier = def.client_interface_name();

    // Stub interface
    {
        let mut stub_interface = Interface::new(interface_identifier.clone());

        // Common used types
        stub_interface.use_type(
            format!("wasi:io/poll@{WASI_WIT_VERSION}"),
            "pollable",
            Some(Ident::new("wasi-io-pollable")),
        );
        stub_interface.use_type(
            format!("wasi:clocks/wall-clock@{WASI_WIT_VERSION}"),
            "datetime",
            Some(Ident::new("wasi-clocks-datetime")),
        );
        stub_interface.use_type(
            format!("golem:rpc/types@{GOLEM_RPC_WIT_VERSION}"),
            "component-id",
            Some(Ident::new("golem-rpc-component-id")),
        );
        stub_interface.use_type(
            format!("golem:rpc/types@{GOLEM_RPC_WIT_VERSION}"),
            "worker-id",
            Some(Ident::new("golem-rpc-worker-id")),
        );
        stub_interface.use_type(
            format!("golem:rpc/types@{GOLEM_RPC_WIT_VERSION}"),
            "cancellation-token",
            Some(Ident::new("golem-rpc-cancellation-token")),
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
        for entity in def.stubbed_entities() {
            for (function, _) in entity.all_functions() {
                if !function.results.is_empty() {
                    add_async_return_type(def, &mut stub_interface, function, entity)?;
                }
            }
        }

        // Function definitions
        for entity in def.stubbed_entities() {
            let mut stub_functions = Vec::<ResourceFunc>::new();

            // Constructors
            {
                let mut constructor = ResourceFunc::constructor();
                let mut params = entity.constructor_params().to_encoder(def)?;

                if !def.config.is_ephemeral {
                    params
                        .items_mut()
                        .insert(0, (Ident::new("worker-name"), Type::String));
                }
                constructor.set_params(params);
                stub_functions.push(constructor);
            }

            {
                let mut custom_constructor = ResourceFunc::static_("custom", false);
                let mut params = entity.constructor_params().to_encoder(def)?;
                params.items_mut().insert(
                    0,
                    (
                        if def.config.is_ephemeral {
                            Ident::new("component-id")
                        } else {
                            Ident::new("worker-id")
                        },
                        Type::Named(Ident::new(if def.config.is_ephemeral {
                            "golem-rpc-component-id"
                        } else {
                            "golem-rpc-worker-id"
                        })),
                    ),
                );
                custom_constructor.set_params(params);
                custom_constructor
                    .set_result(Some(Type::Named(Ident::new(entity.name().to_string()))));
                stub_functions.push(custom_constructor);
            }

            // Functions
            for (function, is_static) in entity.all_functions() {
                // Blocking
                {
                    let mut blocking_function = {
                        let function_name = naming::wit::blocking_function_name(function);
                        if is_static {
                            ResourceFunc::static_(function_name, false)
                        } else {
                            ResourceFunc::method(function_name, false)
                        }
                    };
                    blocking_function.set_params(function.params.to_encoder(def)?);
                    if !function.results.is_empty() {
                        blocking_function.set_result(function.results.to_encoder(def)?);
                    }
                    stub_functions.push(blocking_function);
                }

                // Async
                {
                    let mut async_function = {
                        if is_static {
                            ResourceFunc::static_(function.name.clone(), false)
                        } else {
                            ResourceFunc::method(function.name.clone(), false)
                        }
                    };
                    async_function.set_params(function.params.to_encoder(def)?);
                    if !function.results.is_empty() {
                        async_function.set_result(Some(Type::Named(Ident::new(
                            function.async_result_type(entity),
                        ))));
                    }
                    stub_functions.push(async_function);
                }

                // Scheduled
                {
                    let mut scheduled_function = {
                        let function_name = naming::wit::schedule_function_name(function);
                        if is_static {
                            ResourceFunc::static_(function_name, false)
                        } else {
                            ResourceFunc::method(function_name, false)
                        }
                    };

                    let mut params: Params = function.params.to_encoder(def)?;
                    params.push(
                        Ident::new("scheduled-for"),
                        Type::named(Ident::new("wasi-clocks-datetime")),
                    );
                    scheduled_function.set_params(params);

                    let results = Some(Type::named("golem-rpc-cancellation-token"));
                    scheduled_function.set_result(results);

                    stub_functions.push(scheduled_function);
                }
            }

            stub_interface.type_def(TypeDef::resource(entity.name().to_string(), stub_functions));
        }

        package.interface(stub_interface);
    }

    // Stub world
    {
        let mut stub_world = World::new(def.client_world_name());
        stub_world.named_interface_export(interface_identifier);
        package.world(stub_world);
    }

    Ok(package)
}

fn add_async_return_type(
    def: &StubDefinition,
    stub_interface: &mut Interface,
    function: &FunctionStub,
    owner_entity: &StubbedEntity,
) -> anyhow::Result<()> {
    let context = || {
        anyhow!(
            "Failed to add async return type to stub, stub interface: {}, owner interface: {}, function: {:?}", stub_interface.name(), owner_entity.name(), function.name)
    };

    stub_interface.type_def(TypeDef::resource(
        function.async_result_type(owner_entity),
        [
            {
                let mut subscribe = ResourceFunc::method("subscribe", false);
                subscribe.set_result(Some(Type::Named(Ident::new("wasi-io-pollable"))));
                subscribe
            },
            {
                let mut get = ResourceFunc::method("get", false);
                match &function.results {
                    FunctionResultStub::Anon(typ) => {
                        get.set_result(Some(Type::option(
                            typ.to_encoder(def).with_context(context)?,
                        )));
                    }
                    FunctionResultStub::Unit => {
                        Err(anyhow!("Unit result is not supported as async stub result",))
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
            def.config.client_root.log_color_highlight(),
        ),
    );

    let _indent = LogIndent::new();

    let partial_stub_dep_packages = HashSet::from([
        wit_parser::PackageName {
            namespace: "wasi".to_string(),
            name: "io".to_string(),
            version: Some(Version::from_str(WASI_WIT_VERSION)?),
        },
        wit_parser::PackageName {
            namespace: "wasi".to_string(),
            name: "clocks".to_string(),
            version: Some(Version::from_str(WASI_WIT_VERSION)?),
        },
    ]);

    let stub_dep_packages = def.stub_dep_package_ids();

    let target_wit_root = def.client_wit_root();
    let target_deps = target_wit_root.join(naming::wit::DEPS_DIR);

    for (package_id, package, package_sources) in def.packages_with_wit_sources() {
        if (!stub_dep_packages.contains(&package_id)
            && !partial_stub_dep_packages.contains(&package.name))
            || package_id == def.source_package_id
        {
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

    let pkg_group = UnresolvedPackageGroup::parse_dir(target_wit_root)?;
    let contains_golem_rpc = pkg_group.nested.iter().any(|pkg| {
        pkg.name
            == wit_parser::PackageName {
                namespace: "golem".to_string(),
                name: "rpc".to_string(),
                version: Some(Version::from_str(GOLEM_RPC_WIT_VERSION).unwrap()),
            }
    });
    let contains_wasi_io = pkg_group.nested.iter().any(|pkg| {
        pkg.name
            == wit_parser::PackageName {
                namespace: "wasi".to_string(),
                name: "io".to_string(),
                version: Some(Version::from_str(WASI_WIT_VERSION).unwrap()),
            }
    });
    let contains_wasi_clocks = pkg_group.nested.iter().any(|pkg| {
        pkg.name
            == wit_parser::PackageName {
                namespace: "wasi".to_string(),
                name: "clocks".to_string(),
                version: Some(Version::from_str(WASI_WIT_VERSION).unwrap()),
            }
    });

    if !contains_golem_rpc {
        write_embedded_source(
            &target_deps.join("golem-rpc"),
            &[("wasm-rpc.wit", GOLEM_RPC_WIT)],
        )?;
    }

    if !contains_wasi_io {
        write_embedded_source(&target_deps.join("io"), WASI_IO_WIT)?;
    }

    if !contains_wasi_clocks {
        write_embedded_source(&target_deps.join("clocks"), WASI_CLOCKS_WIT)?;
    }

    Ok(())
}

fn write_embedded_source(target_dir: &Path, contents: &[(&str, &str)]) -> anyhow::Result<()> {
    fs::create_dir_all(target_dir)?;

    for (file_name, content) in contents {
        log_action(
            "Writing",
            format!(
                "{} to {}",
                file_name.log_color_highlight(),
                target_dir.log_color_highlight()
            ),
        );

        fs::write(target_dir.join(file_name), content)?;
    }

    Ok(())
}

#[derive(PartialEq, Eq)]
pub enum UpdateCargoToml {
    Update,
    UpdateIfExists,
    NoUpdate,
}

pub struct AddClientAsDepConfig {
    pub client_wit_root: PathBuf,
    pub dest_wit_root: PathBuf,
    pub update_cargo_toml: UpdateCargoToml,
}

fn can_skip(
    source_resolve: &Resolve,
    target_resolve: &Resolve,
    package_name: &wit_parser::PackageName,
) -> bool {
    if target_resolve.package_names.contains_key(package_name) {
        let source_packages = packages_from_parsed(source_resolve);
        let target_packages = packages_from_parsed(target_resolve);

        let source_package = source_packages
            .iter()
            .find(|pkg| pkg.name().to_string() == package_name.to_string())
            .unwrap();
        let target_package = target_packages
            .iter()
            .find(|pkg| pkg.name().to_string() == package_name.to_string())
            .unwrap();

        let wit_source = source_package.to_string();
        let wit_target = target_package.to_string();

        wit_source == wit_target
    } else {
        false
    }
}

pub fn add_client_as_dependency_to_wit_dir(config: AddClientAsDepConfig) -> anyhow::Result<()> {
    log_action(
        "Adding",
        format!(
            "client dependencies to {} from {}",
            config.dest_wit_root.log_color_highlight(),
            config.client_wit_root.log_color_highlight()
        ),
    );

    let _indent = LogIndent::new();

    let client_resolved_wit_root = ResolvedWitDir::new(&config.client_wit_root)?;
    let client_package = client_resolved_wit_root.main_package()?;

    let dest_resolved_wit_root = ResolvedWitDir::new(&config.dest_wit_root)?;

    let mut dest_encoded_wit_root = EncodedWitDir::new(&dest_resolved_wit_root.resolve)?;

    let mut actions = OverwriteSafeActions::new();
    let mut package_names_to_package_path = BTreeMap::<wit_parser::PackageName, PathBuf>::new();

    for (package_name, package_id) in &client_resolved_wit_root.resolve.package_names {
        let package_sources = client_resolved_wit_root
            .package_sources
            .get(package_id)
            .ok_or_else(|| anyhow!("Failed to get package sources for {}", package_name))?;

        if *package_id == client_resolved_wit_root.package_id {
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
        } else if !can_skip(
            &client_resolved_wit_root.resolve,
            &dest_resolved_wit_root.resolve,
            package_name,
        ) {
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
                        .join(PathExtra::new(&source).strip_prefix(&config.client_wit_root)?),
                });
            }
        } else {
            log_warn_action(
                "Skipping",
                format!(
                    "package dependency {}, already exists in destination",
                    package_name.to_string().log_color_highlight()
                ),
            );
        }
    }

    // Import client and remove source interfaces
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
    //       imports from the source-exports package, given they might have been removed or renamed
    //       in the source, and could create invalid imports.
    remove_world_named_interface_imports(
        package,
        &naming::wit::client_import_exports_prefix_from_client_package_name(&client_package.name)?,
    );
    add_world_named_interface_import(package, &naming::wit::client_import_name(client_package)?);

    let content = package.to_string();

    actions.add(OverwriteSafeAction::WriteFile {
        content,
        target: dest_main_package_sources.files[0].clone(),
    });

    // Check overwrites
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
            wit_parser::Type::ErrorContext => Type::ErrorContext,
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
    type EncoderType = Option<Type>;

    fn to_encoder(&self, def: &StubDefinition) -> anyhow::Result<Self::EncoderType> {
        let context = || {
            anyhow!(
                "Failed to convert function result stub to encoder type: {:?}",
                self
            )
        };
        Ok(match self {
            FunctionResultStub::Anon(typ) => Some(typ.to_encoder(def).with_context(context)?),
            FunctionResultStub::Unit => None,
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

pub fn extract_exports_as_wit_dep(wit_dir: &Path) -> anyhow::Result<()> {
    log_action(
        "Extracting",
        format!(
            "exports package from main package in wit directory {}",
            wit_dir.log_color_highlight()
        ),
    );

    let resolved_wit_dir = ResolvedWitDir::new(wit_dir)?;
    let main_package_id = resolved_wit_dir.package_id;
    let mut encoded_wit_dir = EncodedWitDir::new(&resolved_wit_dir.resolve)?;

    let (main_package, exports_package) =
        extract_exports_package(main_package_id, &mut encoded_wit_dir)?;
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

    let exports_package_path = wit_dir
        .join(naming::wit::DEPS_DIR)
        .join(package_dep_dir_name_from_encoder(exports_package.name()))
        .join(naming::wit::EXPORTS_WIT_FILE_NAME);
    log_action(
        "Writing",
        format!(
            "exports package to {}",
            exports_package_path.log_color_highlight()
        ),
    );
    fs::write_str(&exports_package_path, exports_package.to_string())?;

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
fn extract_exports_package(
    main_package_id: PackageId,
    encoded_wit_dir: &mut EncodedWitDir,
) -> anyhow::Result<(Package, Package)> {
    let package = encoded_wit_dir.package(main_package_id)?;

    let mut exports_package = package.clone();
    exports_package.set_name(naming::wit::exports_encoder_package_name(package.name()));

    let exports_prefix = format!(
        "{}:{}/",
        package.name().namespace(),
        exports_package.name().name()
    );
    let exports_suffix = package
        .name()
        .version()
        .map(|version| format!("@{version}"))
        .unwrap_or_default();

    let mut exported_interface_identifiers = HashSet::<Ident>::new();
    let mut inline_interface_exports = BTreeMap::<Ident, BTreeMap<Ident, Interface>>::new();
    let mut inline_function_exports = BTreeMap::<Ident, Vec<StandaloneFunc>>::new();
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            let world_name = world.name().clone();
            let inline_interface_exports = inline_interface_exports
                .entry(world_name.clone())
                .or_default();
            let inline_function_exports = inline_function_exports
                .entry(world_name.clone())
                .or_default();

            world.items_mut().retain(|world_item| match world_item {
                // Remove and collect inline interface exports
                WorldItem::InlineInterfaceExport(interface) => {
                    let mut interface = interface.clone();
                    let interface_name = interface.name().clone();
                    interface.set_name(naming::wit::exports_package_world_inline_interface_name(
                        &world_name,
                        interface.name(),
                    ));
                    inline_interface_exports.insert(interface_name, interface);
                    false
                }
                // Remove and collect inline function exports
                WorldItem::FunctionExport(function) => {
                    inline_function_exports.push(function.clone());
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
            for interface in inline_interface_exports.values_mut() {
                world.named_interface_export(interface.name().clone());
            }

            // Insert named import for extracted inline functions
            if !inline_function_exports.is_empty() {
                world.named_interface_export(format!(
                    "{}{}{}",
                    exports_prefix,
                    naming::wit::interface_package_world_inline_functions_interface_name(
                        &world_name
                    ),
                    exports_suffix
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

    // Rename named self export and imports to use the extracted interface names
    for package_item in package.items_mut() {
        if let PackageItem::World(world) = package_item {
            for world_item in world.items_mut() {
                if let WorldItem::NamedInterfaceImport(import) = world_item {
                    if !import.name().raw_name().contains("/") {
                        import.set_name(format!(
                            "{}{}{}",
                            exports_prefix,
                            import.name(),
                            exports_suffix
                        ));
                    }
                } else if let WorldItem::NamedInterfaceExport(export) = world_item {
                    if !export.name().raw_name().contains("/") {
                        export.set_name(format!(
                            "{}{}{}",
                            exports_prefix,
                            export.name(),
                            exports_suffix
                        ));
                    }
                }
            }
        }
    }

    // Collect type identifiers used by inlined items
    let mut inline_interfaces_used_type_identifiers =
        HashMap::<Ident, HashMap<Ident, HashSet<Ident>>>::new();
    let mut inline_functions_used_type_identifiers = HashMap::<Ident, HashSet<Ident>>::new();
    for (world_name, inline_interface_exports) in &inline_interface_exports {
        let identifiers_by_interfaces = inline_interfaces_used_type_identifiers
            .entry(world_name.clone())
            .or_default();
        for (interface_name, interface) in inline_interface_exports {
            identifiers_by_interfaces.insert(interface_name.clone(), interface.used_type_idents());
        }
    }
    for (world_name, inline_function_exports) in &inline_function_exports {
        inline_functions_used_type_identifiers
            .entry(world_name.clone())
            .or_default()
            .extend(
                inline_function_exports
                    .iter()
                    .flat_map(|function| function.used_type_idents()),
            );
    }

    let mut inline_function_uses = HashMap::<Ident, Vec<Use>>::new();
    exports_package.items_mut().retain(|item| match item {
        // Drop non-exported interfaces from exports package
        PackageItem::Interface(interface) => {
            exported_interface_identifiers.contains(interface.name())
        }
        // Drop all worlds from exports package, while moving export used "uses" into the new interfaces
        PackageItem::World(world) => {
            let use_to_idents = world
                .uses()
                .iter()
                .map(|use_| {
                    (
                        use_,
                        use_.use_names_list()
                            .iter()
                            .map(|(name, alias)| alias.as_ref().unwrap_or(name))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<HashMap<_, _>>();

            // Copy used uses into interfaces
            {
                let inline_interface_exports = inline_interface_exports
                    .get_mut(world.name())
                    .expect("Missing world in inline_interface_exports");

                let inline_interfaces_used_type_identifiers =
                    inline_interfaces_used_type_identifiers
                        .get(world.name())
                        .expect("Missing world in inline_interfaces_used_type_identifiers");

                for (interface_name, used_type_identifiers) in
                    inline_interfaces_used_type_identifiers
                {
                    let interface = inline_interface_exports
                        .get_mut(interface_name)
                        .expect("Missing interface in inline_interface_exports");
                    use_to_idents
                        .iter()
                        .filter(|(_, idents)| {
                            idents
                                .iter()
                                .any(|ident| used_type_identifiers.contains(ident))
                        })
                        .for_each(|(&use_, _)| interface.use_(use_.clone()))
                }
            }

            // Copy used uses into function interface
            {
                let uses = inline_function_uses
                    .entry(world.name().clone())
                    .or_default();

                let inline_functions_used_type_identifiers = inline_functions_used_type_identifiers
                    .get(world.name())
                    .expect("Missing world in inline_functions_used_type_identifiers");

                use_to_idents
                    .iter()
                    .filter(|(_, idents)| {
                        idents
                            .iter()
                            .any(|ident| inline_functions_used_type_identifiers.contains(ident))
                    })
                    .for_each(|(&use_, _)| uses.push(use_.clone()))
            }

            false
        }
    });

    // Add inlined exported interfaces to the exports package
    for (_, interfaces) in inline_interface_exports {
        for (_, interface) in interfaces {
            exports_package.interface(interface);
        }
    }

    // Add interface for inlined functions to the exports package
    for (world_name, functions) in inline_function_exports {
        let mut interface = Interface::new(
            naming::wit::interface_package_world_inline_functions_interface_name(&world_name),
        );

        for use_ in inline_function_uses.remove(&world_name).unwrap_or_default() {
            interface.use_(use_);
        }

        for function in functions {
            interface.function(function);
        }

        exports_package.interface(interface);
    }

    Ok((package.clone(), exports_package))
}

pub struct ExtractWasmInterfaceResult {
    pub new_packages: BTreeSet<PackageName>,
    pub required_common_packages: BTreeSet<PackageName>,
}

pub fn extract_wasm_interface_as_wit_dep(
    common_deps: &WitDepsResolver,
    dep_name: &str,
    wasm: &Path,
    wit_dir: &Path,
) -> anyhow::Result<ExtractWasmInterfaceResult> {
    log_action(
        "Extracting",
        format!(
            "WIT package from WASM library dependency {} into wit directory {}",
            dep_name.log_color_highlight(),
            wit_dir.log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();

    let component_bytes = std::fs::read(wasm)?;
    let wasm = wit_parser::decoding::decode(&component_bytes)?;

    let mut result = ExtractWasmInterfaceResult {
        new_packages: BTreeSet::new(),
        required_common_packages: BTreeSet::new(),
    };
    let pkg_group = UnresolvedPackageGroup::parse_dir(wit_dir)?;

    for (id, package) in wasm.resolve().packages.iter() {
        let contains = pkg_group.nested.iter().any(|pkg| pkg.name == package.name);

        if !contains {
            if common_deps.package(&package.name).is_ok() {
                log_action(
                    "Copying",
                    format!(
                        "package {} the common WIT directory for dependency {}",
                        package.name.to_string().log_color_highlight(),
                        dep_name.log_color_highlight(),
                    ),
                );

                result.required_common_packages.insert(package.name.clone());
            } else {
                log_action(
                    "Adding",
                    format!(
                        "package {} from dependency {}",
                        package.name.to_string().log_color_highlight(),
                        dep_name.log_color_highlight(),
                    ),
                );

                let target_dir = wit_dir
                    .join(naming::wit::DEPS_DIR)
                    .join(package_dep_dir_name_from_parser(&package.name));
                fs::create_dir_all(&target_dir)?;
                let file_name = format!("{}.wit", package.name.name);

                let mut wit_printer = WitPrinter::default();
                wit_printer.emit_docs(true);

                wit_printer.print(wasm.resolve(), id, &[])?;
                std::fs::write(target_dir.join(file_name), wit_printer.output.to_string())?;

                result.new_packages.insert(package.name.clone());
            }
        } else {
            log_action(
                "Skipping",
                format!(
                    "package {} from dependency {}",
                    package.name.to_string().log_color_highlight(),
                    dep_name.log_color_highlight()
                ),
            );
        }
    }

    Ok(result)
}

trait UsedTypeIdents {
    fn used_type_idents(&self) -> HashSet<Ident>;
}

impl UsedTypeIdents for Type {
    fn used_type_idents(&self) -> HashSet<Ident> {
        match self {
            Type::Bool => HashSet::new(),
            Type::U8 => HashSet::new(),
            Type::U16 => HashSet::new(),
            Type::U32 => HashSet::new(),
            Type::U64 => HashSet::new(),
            Type::S8 => HashSet::new(),
            Type::S16 => HashSet::new(),
            Type::S32 => HashSet::new(),
            Type::S64 => HashSet::new(),
            Type::F32 => HashSet::new(),
            Type::F64 => HashSet::new(),
            Type::Char => HashSet::new(),
            Type::String => HashSet::new(),
            Type::Borrow(ident) => HashSet::from([ident.clone()]),
            Type::Option(type_) => type_.used_type_idents(),
            Type::Result(result_) => {
                let mut idents = HashSet::<Ident>::new();
                idents.extend(
                    result_
                        .get_ok()
                        .as_ref()
                        .map(|type_| type_.used_type_idents())
                        .unwrap_or_default(),
                );
                idents.extend(
                    result_
                        .get_err()
                        .as_ref()
                        .map(|type_| type_.used_type_idents())
                        .unwrap_or_default(),
                );
                idents
            }
            Type::List(type_) => type_.used_type_idents(),
            Type::FixedSizeList(type_, _) => type_.used_type_idents(),
            Type::Tuple(tuple) => tuple
                .types()
                .iter()
                .flat_map(|type_| type_.used_type_idents())
                .collect(),
            Type::Named(ident) => HashSet::from([ident.clone()]),
            Type::Future(_) => {
                panic!("WASI P3 Future is not supported yet")
            }
            Type::Stream(_) => {
                panic!("WASI P3 Stream is not supported yet")
            }
            Type::ErrorContext => {
                panic!("WASI P3 ErrorContext is not supported yet")
            }
        }
    }
}

impl UsedTypeIdents for TypeDef {
    fn used_type_idents(&self) -> HashSet<Ident> {
        match self.kind() {
            TypeDefKind::Record(record) => record
                .fields()
                .iter()
                .flat_map(|field| field.type_().used_type_idents())
                .collect(),
            TypeDefKind::Resource(resource) => resource
                .funcs()
                .iter()
                .flat_map(|func| func.used_type_idents())
                .collect(),
            TypeDefKind::Flags(_) => HashSet::new(),
            TypeDefKind::Variant(variant) => variant
                .cases()
                .iter()
                .flat_map(|case| {
                    case.type_()
                        .iter()
                        .flat_map(|type_| type_.used_type_idents())
                        .collect::<HashSet<_>>()
                })
                .collect(),
            TypeDefKind::Enum(_) => HashSet::new(),
            TypeDefKind::Type(type_) => type_.used_type_idents(),
        }
    }
}

impl UsedTypeIdents for Params {
    fn used_type_idents(&self) -> HashSet<Ident> {
        self.items()
            .iter()
            .flat_map(|(_, type_)| type_.used_type_idents())
            .collect()
    }
}

impl UsedTypeIdents for ResourceFunc {
    fn used_type_idents(&self) -> HashSet<Ident> {
        let mut idents = HashSet::<Ident>::new();
        idents.extend(self.params().used_type_idents());

        let results = match self.kind() {
            ResourceFuncKind::Method(_, _, results) => Some(results),
            ResourceFuncKind::Static(_, _, results) => Some(results),
            ResourceFuncKind::Constructor => None,
        };
        if let Some(results) = results {
            idents.extend(results.used_type_idents());
        }

        idents
    }
}

impl UsedTypeIdents for Option<Type> {
    fn used_type_idents(&self) -> HashSet<Ident> {
        match self {
            None => HashSet::new(),
            Some(type_) => type_.used_type_idents(),
        }
    }
}

impl UsedTypeIdents for InterfaceItem {
    fn used_type_idents(&self) -> HashSet<Ident> {
        match self {
            InterfaceItem::TypeDef(type_def) => type_def.used_type_idents(),
            InterfaceItem::Function(function) => {
                let mut idents = HashSet::<Ident>::new();
                idents.extend(function.params().used_type_idents());
                idents.extend(function.result().used_type_idents());
                idents
            }
        }
    }
}

impl UsedTypeIdents for Interface {
    fn used_type_idents(&self) -> HashSet<Ident> {
        self.items()
            .iter()
            .flat_map(|item| item.used_type_idents())
            .collect()
    }
}

impl UsedTypeIdents for StandaloneFunc {
    fn used_type_idents(&self) -> HashSet<Ident> {
        let mut idents = HashSet::<Ident>::new();
        idents.extend(self.params().used_type_idents());
        idents.extend(self.result().used_type_idents());
        idents
    }
}
