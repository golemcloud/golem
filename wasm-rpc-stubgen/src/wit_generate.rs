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

use crate::commands::log::{log_action, log_warn_action, LogIndent};
use crate::fs::{copy, copy_transformed, get_file_name};
use crate::naming;
use crate::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, InterfaceStub, InterfaceStubTypeDef,
    StubConfig, StubDefinition,
};
use crate::wit_transform::import_remover;
use std::fs;
use std::path::Path;
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

pub fn add_wit_dependencies(def: &StubDefinition) -> anyhow::Result<()> {
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
                    "  Copying",
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
                    "  Copying",
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
