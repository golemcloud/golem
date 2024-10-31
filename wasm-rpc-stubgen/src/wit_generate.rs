// Copyright 2024 Golem Cloud
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

use crate::commands::log::{log_action, log_action_plan, log_warn_action, LogIndent};
use crate::fs::{
    copy, copy_transformed, get_file_name, strip_path_prefix, OverwriteSafeAction,
    OverwriteSafeActions,
};
use crate::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, InterfaceStub, InterfaceStubTypeDef,
    StubConfig, StubDefinition,
};
use crate::wit_encode::EncodedWitDir;
use crate::wit_resolve::{PackageSource, ResolvedWitDir};
use crate::wit_transform::{
    add_world_named_interface_import, import_remover, world_named_interface_import_remover,
};
use crate::{cargo, naming};
use anyhow::{anyhow, bail, Context};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use wit_encoder::{
    Ident, Interface, Package, PackageName, Params, ResourceFunc, Results, Type, TypeDef,
    TypeDefKind, VariantCase, World,
};

pub fn generate_stub_wit_to_target(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Generating",
        format!("stub WIT to {}", def.target_wit_path().display()),
    );

    let out = generate_stub_wit_from_stub_def(def)?;
    fs::create_dir_all(def.target_wit_root())?;
    fs::write(def.target_wit_path(), out)?;
    Ok(())
}

pub fn generate_stub_wit_from_wit_dir(
    source_wit_root: &Path,
    inline_source_types: bool,
) -> anyhow::Result<String> {
    generate_stub_wit_from_stub_def(&StubDefinition::new(StubConfig {
        source_wit_root: source_wit_root.to_path_buf(),
        target_root: source_wit_root.to_path_buf(), // Not used
        selected_world: None,                       // TODO: would this fail with multiple worlds?
        stub_crate_version: "".to_string(),         // Not used
        wasm_rpc_override: Default::default(),      // Not used
        inline_source_types,
    })?)
}

pub fn generate_stub_wit_from_stub_def(def: &StubDefinition) -> anyhow::Result<String> {
    Ok(generate_stub_package_from_stub_def(def).to_string())
}

pub fn generate_stub_package_from_stub_def(def: &StubDefinition) -> Package {
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
            if type_def.inlined {
                stub_interface.type_def(type_def.to_encoder(def))
            } else {
                stub_interface.use_type(
                    type_def.interface_identifier.clone(),
                    type_def.type_name.clone(),
                    type_def.type_name_alias.clone().map(Ident::from),
                );
            }
        }

        // Async return types
        for interface in def.stub_imported_interfaces() {
            for (function, _) in interface.all_functions() {
                if !function.results.is_empty() {
                    add_async_return_type(def, &mut stub_interface, function, interface);
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
                    Some(constructor_params) => constructor_params.to_encoder(def),
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
                    blocking_function.set_params(function.params.to_encoder(def));
                    if !function.results.is_empty() {
                        blocking_function.set_results(function.results.to_encoder(def));
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
                    async_function.set_params(function.params.to_encoder(def));
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

    package
}

fn add_async_return_type(
    def: &StubDefinition,
    stub_interface: &mut Interface,
    function: &FunctionStub,
    owner_interface: &InterfaceStub,
) {
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
                        get.set_results(Results::Anon(Type::option(typ.to_encoder(def))));
                    }
                    FunctionResultStub::Named(_params) => {
                        // TODO: this case was generating in the previous version, check it
                        // TODO: this should be a proper error message with context
                        panic!("Named parameters are not supported");
                    }
                    FunctionResultStub::SelfType => {
                        // TODO: this should be a proper error message with context
                        panic!("Unexpected self return type in wit generator");
                    }
                }
                get
            },
        ],
    ))
}

pub fn add_dependencies_to_stub_wit_dir(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Adding",
        format!(
            "WIT dependencies from {} to {}",
            def.config.source_wit_root.display(),
            def.config.target_root.display()
        ),
    );

    let stub_package_name = def.stub_package_name();
    let stub_dep_packages = def.stub_dep_package_ids();
    let remove_stub_imports = import_remover(&stub_package_name);

    let target_wit_root = def.target_wit_root();
    let target_deps = target_wit_root.join(naming::wit::DEPS_DIR);

    let _indent = LogIndent::new();
    for (package_id, package, package_sources) in def.packages_with_wit_sources() {
        if !stub_dep_packages.contains(&package_id) {
            log_warn_action(
                "Skipping",
                format!(
                    "package dependency {}, not used in the stub interface",
                    package.name
                ),
            );
            continue;
        }

        let is_source_package = package.name == def.source_package_name;

        log_action("Copying", format!("package dependency {}", package.name));

        let _indent = LogIndent::new();
        for source in &package_sources.files {
            if is_source_package {
                let dest = target_deps
                    .join(naming::wit::package_dep_dir_name(&def.source_package_name))
                    .join(get_file_name(source)?);
                log_action(
                    "Copying",
                    format!(
                        "(with source imports removed) {} to {}",
                        source.display(),
                        dest.display()
                    ),
                );
                copy_transformed(source, &dest, &remove_stub_imports)?;
            } else {
                let relative = source.strip_prefix(&def.config.source_wit_root)?;
                let dest = target_wit_root.join(relative);
                log_action(
                    "Copying",
                    format!("{} to {}", source.display(), dest.display()),
                );
                copy(source, &dest)?;
            }
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
        format!("{} to {}", file_name, target_dir.display()),
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
    pub overwrite: bool,
    pub remove_dest_imports: bool,
    pub update_cargo_toml: UpdateCargoToml,
}

// TODO: "named" args?
pub fn add_stub_as_dependency_to_wit_dir(config: AddStubAsDepConfig) -> anyhow::Result<()> {
    log_action(
        "Adding",
        format!(
            "stub dependencies to {} from {}",
            config.dest_wit_root.display(),
            config.stub_wit_root.display()
        ),
    );

    let _indent = LogIndent::new();

    let stub_resolved_wit_root = ResolvedWitDir::new(&config.stub_wit_root)?;
    let stub_package = stub_resolved_wit_root.main_package()?;
    let stub_wit = config.stub_wit_root.join(naming::wit::STUB_WIT_FILE_NAME);

    let dest_deps_dir = config.dest_wit_root.join(naming::wit::DEPS_DIR);
    let dest_resolved_wit_root = ResolvedWitDir::new(&config.dest_wit_root)?;
    let dest_package = dest_resolved_wit_root.main_package()?;
    let dest_stub_package_name = naming::wit::stub_package_name(&dest_package.name);

    // TODO: these could be made optional depending on remove_dest_imports
    let mut stub_encoded_wit_root = EncodedWitDir::new(&stub_resolved_wit_root.resolve)?;
    let remove_dest_package_import = world_named_interface_import_remover(&dest_package.name);
    let remove_dest_stub_package_import =
        world_named_interface_import_remover(&dest_stub_package_name);

    // TODO: this could be made optional depending on "add stub import" (=overwrite)
    let mut dest_encoded_wit_root = EncodedWitDir::new(&dest_resolved_wit_root.resolve)?;

    // TODO: make this optional (not needed for declarative)
    {
        let is_self_stub_by_name =
            dest_package.name == naming::wit::stub_target_package_name(&stub_package.name);
        let is_self_stub_by_content = is_self_stub(&stub_wit, &config.dest_wit_root);

        if is_self_stub_by_name && !is_self_stub_by_content? {
            return Err(anyhow!(
            "Both the caller and the target components are using the same package name ({}), which is not supported.",
            dest_package.name
        ));
        }
    }

    let mut actions = OverwriteSafeActions::new();
    let mut package_names_to_package_path = BTreeMap::<wit_parser::PackageName, PathBuf>::new();

    for (package_name, package_id) in &stub_resolved_wit_root.resolve.package_names {
        let package_sources = stub_resolved_wit_root
            .package_sources
            .get(package_id)
            .ok_or_else(|| anyhow!("Failed to get package sources for {}", package_name))?;
        let package_path = naming::wit::package_wit_dep_dir_from_package_dir_name(&get_file_name(
            &package_sources.dir,
        )?);

        let is_stub_main_package = *package_id == stub_resolved_wit_root.package_id;
        let is_dest_package = *package_name == dest_package.name;
        let is_dest_stub_package = *package_name == dest_stub_package_name;

        // We skip self as a dependency
        if is_dest_package {
            log_warn_action(
                "Skipping",
                format!("cyclic self dependency for {}", package_name),
            );
        } else if is_dest_stub_package || is_stub_main_package {
            let package_dep_dir_name = naming::wit::package_dep_dir_name(package_name);
            let package_path = naming::wit::package_wit_dep_dir_from_package_name(package_name);

            package_names_to_package_path.insert(package_name.clone(), package_path);

            // Handle self stub packages: use regenerated stub with inlining, to break the recursive cycle
            if is_dest_stub_package {
                actions.add(OverwriteSafeAction::WriteFile {
                    // TODO: this call potentially builds another EncodedWitDir for source
                    content: generate_stub_wit_from_wit_dir(&config.dest_wit_root, true)?,
                    target: dest_deps_dir
                        .join(&package_dep_dir_name)
                        .join(naming::wit::STUB_WIT_FILE_NAME),
                });
            // Non-self stub package has to be copied into target deps
            } else {
                for source in &package_sources.files {
                    actions.add(OverwriteSafeAction::CopyFile {
                        source: source.clone(),
                        target: dest_deps_dir
                            .join(&package_dep_dir_name)
                            .join(get_file_name(source)?),
                    });
                }
            }
        // Handle other package by copying and removing cyclic imports
        } else if config.remove_dest_imports {
            package_names_to_package_path.insert(package_name.clone(), package_path);

            let package = stub_encoded_wit_root.package(*package_id)?;
            remove_dest_package_import(package);
            remove_dest_stub_package_import(package);
            let content = package.to_string();

            let target = wit_dep_transformed_target_file(
                &config.stub_wit_root,
                &config.dest_wit_root,
                package_name,
                package_sources,
            )?;

            actions.add(OverwriteSafeAction::WriteFile { content, target });
        // Handle other package by copying and no transformations
        } else {
            for source in &package_sources.files {
                actions.add(OverwriteSafeAction::CopyFile {
                    source: source.clone(),
                    target: config
                        .dest_wit_root
                        .join(strip_path_prefix(&config.stub_wit_root, source)?),
                });
            }
        }
    }

    // Import stub if overwrite enabled // TODO: use a different flag, or always import?
    if config.overwrite {
        let dest_main_package_id = dest_resolved_wit_root.package_id;

        let dest_main_package_sources = dest_resolved_wit_root
            .package_sources
            .get(&dest_main_package_id)
            .ok_or_else(|| anyhow!("Failed to get dest main package sources"))?;

        if dest_main_package_sources.files.len() != 1 {
            bail!(
                "Expected exactly one dest main package source, got sources: {:?}",
                dest_main_package_sources.files
            );
        }

        let package = dest_encoded_wit_root.package(dest_main_package_id)?;
        add_world_named_interface_import(package, &naming::wit::stub_import_name(stub_package)?);
        let content = package.to_string();

        actions.add(OverwriteSafeAction::WriteFile {
            content,
            target: dest_main_package_sources.files[0].clone(),
        });
    }

    let forbidden_overwrites = actions.run(config.overwrite, log_action_plan)?;
    if !forbidden_overwrites.is_empty() {
        eprintln!("The following files would have been overwritten with new content:");
        for action in forbidden_overwrites {
            eprintln!("  {}", action.target().display());
        }
        eprintln!();
        eprintln!("Use --overwrite to force overwrite.");
    }

    if let Some(target_parent) = config.dest_wit_root.parent() {
        let target_cargo_toml = target_parent.join("Cargo.toml");
        if target_cargo_toml.exists() && target_cargo_toml.is_file() {
            if config.update_cargo_toml == UpdateCargoToml::NoUpdate {
                eprintln!("Warning: the newly copied dependencies have to be added to {}. Use the --update-cargo-toml flag to update it automatically.", target_cargo_toml.display());
            } else {
                cargo::is_cargo_component_toml(&target_cargo_toml).context(format!(
                    "The file {target_cargo_toml:?} is not a valid cargo-component project"
                ))?;
                cargo::add_dependencies_to_cargo_toml(
                    &target_cargo_toml,
                    package_names_to_package_path,
                )?;
            }
        } else if config.update_cargo_toml == UpdateCargoToml::Update {
            return Err(anyhow!(
                "Cannot update {:?} file because it does not exist or is not a file",
                target_cargo_toml
            ));
        }
    } else if config.update_cargo_toml == UpdateCargoToml::Update {
        return Err(anyhow!("Cannot update the Cargo.toml file because parent directory of the destination WIT root does not exist."));
    }

    Ok(())
}

// TODO: let's find another way to identify self stubs, as this is quite diverging now based on content
/// Checks whether `stub_wit` is a stub generated for `dest_wit_root`
fn is_self_stub(stub_wit: &Path, dest_wit_root: &Path) -> anyhow::Result<bool> {
    // TODO: can we make it diff exports instead of generated content?
    let dest_stub_wit_imported = generate_stub_wit_from_wit_dir(dest_wit_root, false)?;
    let dest_stub_wit_inlined = generate_stub_wit_from_wit_dir(dest_wit_root, true)?;
    let stub_wit = std::fs::read_to_string(stub_wit)?;

    // TODO: this can also be false in case the stub is lagging
    Ok(stub_wit == dest_stub_wit_imported || stub_wit == dest_stub_wit_inlined)
}

fn wit_dep_transformed_target_file(
    source_wit_root: &Path,
    target_wit_root: &Path,
    package_name: &wit_parser::PackageName,
    package_sources: &PackageSource,
) -> anyhow::Result<PathBuf> {
    let first_source = package_sources.files.first().ok_or_else(|| {
        anyhow!(
            "Expected at least one source for wit package: {}",
            package_name
        )
    })?;
    let first_source_relative_path = strip_path_prefix(source_wit_root, first_source)?;

    if package_sources.files.len() == 1 {
        Ok(target_wit_root.join(first_source_relative_path))
    } else {
        Ok(target_wit_root
            .join(first_source_relative_path.parent().ok_or_else(|| {
                anyhow!(
                    "Failed to get parent of wit source: {}",
                    first_source_relative_path.display()
                )
            })?)
            .join(naming::wit::package_merged_wit_name(package_name)))
    }
}

trait ToEncoder {
    type EncoderType;
    fn to_encoder(&self, stub_definition: &StubDefinition) -> Self::EncoderType;
}

impl ToEncoder for wit_parser::Type {
    type EncoderType = Type;

    fn to_encoder(&self, def: &StubDefinition) -> Self::EncoderType {
        match self {
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
                    let typ = def.get_type_def(*type_id).expect("type not found");
                    match &typ.kind {
                        wit_parser::TypeDefKind::Option(inner) => {
                            Type::option(inner.to_encoder(def))
                        }
                        wit_parser::TypeDefKind::List(inner) => Type::list(inner.to_encoder(def)),
                        wit_parser::TypeDefKind::Tuple(tuple) => {
                            Type::tuple(tuple.types.iter().map(|t| t.to_encoder(def)))
                        }
                        wit_parser::TypeDefKind::Result(result) => {
                            match (&result.ok, &result.err) {
                                (Some(ok), Some(err)) => {
                                    Type::result_both(ok.to_encoder(def), err.to_encoder(def))
                                }
                                (Some(ok), None) => Type::result_ok(ok.to_encoder(def)),
                                (None, Some(err)) => Type::result_err(err.to_encoder(def)),
                                (None, None) => {
                                    panic!("result type has no ok or err types")
                                }
                            }
                        }
                        wit_parser::TypeDefKind::Handle(handle) => match handle {
                            wit_parser::Handle::Own(type_id) => {
                                wit_parser::Type::Id(*type_id).to_encoder(def)
                            }
                            wit_parser::Handle::Borrow(type_id) => Type::borrow(
                                wit_parser::Type::Id(*type_id).to_encoder(def).to_string(),
                            ),
                        },
                        _ => {
                            let name = typ
                                .name
                                .clone()
                                .unwrap_or_else(|| panic!("missing name for type: {:?}", typ));
                            Type::Named(Ident::new(name))
                        }
                    }
                }
            }
        }
    }
}

impl ToEncoder for Vec<FunctionParamStub> {
    type EncoderType = Params;

    fn to_encoder(&self, def: &StubDefinition) -> Self::EncoderType {
        Params::from_iter(
            self.iter()
                .map(|param| (param.name.clone(), param.typ.to_encoder(def))),
        )
    }
}

impl ToEncoder for FunctionResultStub {
    type EncoderType = Results;

    fn to_encoder(&self, def: &StubDefinition) -> Self::EncoderType {
        match self {
            FunctionResultStub::Anon(typ) => Results::Anon(typ.to_encoder(def)),
            FunctionResultStub::Named(types) => Results::Named(types.to_encoder(def)),
            FunctionResultStub::SelfType => {
                // TODO: This should be a proper error message
                panic!("Unexpected self type function result")
            }
        }
    }
}

impl ToEncoder for InterfaceStubTypeDef {
    type EncoderType = TypeDef;

    fn to_encoder(&self, def: &StubDefinition) -> Self::EncoderType {
        (self.stub_type_name(), self.stub_type_def()).to_encoder(def)
    }
}

impl ToEncoder for (&str, &wit_parser::TypeDef) {
    type EncoderType = TypeDef;

    fn to_encoder(&self, def: &StubDefinition) -> Self::EncoderType {
        let (name, type_def) = self;
        match &type_def.kind {
            wit_parser::TypeDefKind::Record(record) => TypeDef::record(
                name.to_string(),
                record
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.to_encoder(def))),
            ),
            wit_parser::TypeDefKind::Flags(flags) => TypeDef::flags(
                name.to_string(),
                flags.flags.iter().map(|flag| (flag.name.clone(),)),
            ),
            wit_parser::TypeDefKind::Tuple(tuple) => TypeDef::new(
                name.to_string(),
                TypeDefKind::Type(Type::tuple(
                    tuple.types.iter().map(|typ| typ.to_encoder(def)),
                )),
            ),
            wit_parser::TypeDefKind::Variant(variant) => TypeDef::variant(
                name.to_string(),
                variant.cases.iter().map(|case| match case.ty {
                    Some(ty) => VariantCase::value(case.name.clone(), ty.to_encoder(def)),
                    None => VariantCase::empty(case.name.clone()),
                }),
            ),
            wit_parser::TypeDefKind::Enum(enum_ty) => TypeDef::enum_(
                name.to_string(),
                enum_ty.cases.iter().map(|case| case.name.clone()),
            ),
            wit_parser::TypeDefKind::Option(typ) => TypeDef::new(
                name.to_string(),
                TypeDefKind::Type(Type::option(typ.to_encoder(def))),
            ),
            wit_parser::TypeDefKind::Result(result) => TypeDef::new(
                name.to_string(),
                // TODO: match is duplicated
                TypeDefKind::Type(match (&result.ok, &result.err) {
                    (Some(ok), Some(err)) => {
                        Type::result_both(ok.to_encoder(def), err.to_encoder(def))
                    }
                    (Some(ok), None) => Type::result_ok(ok.to_encoder(def)),
                    (None, Some(err)) => Type::result_err(err.to_encoder(def)),
                    (None, None) => {
                        panic!("result type has no ok or err types")
                    }
                }),
            ),
            wit_parser::TypeDefKind::List(typ) => TypeDef::new(
                name.to_string(),
                TypeDefKind::Type(Type::list(typ.to_encoder(def))),
            ),
            wit_parser::TypeDefKind::Future(_) => {
                panic!("Future type is not supported yet")
            }
            wit_parser::TypeDefKind::Stream(_) => {
                panic!("Stream type is not supported yet")
            }
            wit_parser::TypeDefKind::Type(typ) => {
                TypeDef::new(name.to_string(), TypeDefKind::Type(typ.to_encoder(def)))
            }
            wit_parser::TypeDefKind::Unknown => {
                panic!("Unexpected unknown type")
            }
            wit_parser::TypeDefKind::Resource => {
                panic!("Unexpected resource type")
            }
            wit_parser::TypeDefKind::Handle(handle) => TypeDef::new(
                name.to_string(),
                // TODO: match is duplicated
                TypeDefKind::Type(match handle {
                    wit_parser::Handle::Own(type_id) => {
                        wit_parser::Type::Id(*type_id).to_encoder(def)
                    }
                    wit_parser::Handle::Borrow(type_id) => {
                        Type::borrow(wit_parser::Type::Id(*type_id).to_encoder(def).to_string())
                    }
                }),
            ),
        }
    }
}
