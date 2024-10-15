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

use crate::commands::log::{log_action, log_warn_action};
use crate::fs::{copy, copy_transformed};
use crate::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, InterfaceStub, InterfaceStubImport,
    InterfaceStubTypeDef, StubDefinition,
};
use crate::{naming, WasmRpcOverride};
use anyhow::{anyhow, bail};
use indexmap::IndexMap;
use regex::Regex;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use wit_parser::{
    Enum, Field, Flags, Handle, PackageName, Result_, Tuple, Type, TypeDef, TypeDefKind, Variant,
};

pub fn generate_stub_wit_to_target(def: &StubDefinition) -> anyhow::Result<()> {
    log_action(
        "Generating",
        format!("stub WIT to {}", def.target_wit_path().to_string_lossy()),
    );

    let out = generate_stub_wit_from_stub_def(def)?;
    fs::create_dir_all(def.target_wit_root())?;
    fs::write(def.target_wit_path(), out)?;
    Ok(())
}

pub fn generate_stub_wit_from_wit_dir(
    source_wit_root: &Path,
    inline_root_types: bool,
) -> anyhow::Result<String> {
    generate_stub_wit_from_stub_def(&StubDefinition::new(
        source_wit_root,
        source_wit_root,             // Not used
        &None,                       // Not used
        "0.0.1",                     // Not used
        &WasmRpcOverride::default(), // Not used
        inline_root_types,
    )?)
}

pub fn generate_stub_wit_from_stub_def(def: &StubDefinition) -> anyhow::Result<String> {
    let world = def.source_world();

    let mut out = String::new();

    writeln!(out, "package {}-stub;", def.source_package_name)?;
    writeln!(out)?;
    writeln!(out, "interface stub-{} {{", world.name)?;

    let all_imports = def
        .source_interfaces()
        .iter()
        .flat_map(|i| i.imports.iter().map(|i| (InterfaceStubImport::from(i), i)))
        .collect::<IndexMap<_, _>>();

    // Renaming the mandatory imports to avoid collisions with types coming from the stubbed package
    writeln!(out, "  use golem:rpc/types@0.1.0.{{uri as golem-rpc-uri}};")?;
    writeln!(
        out,
        "  use wasi:io/poll@0.2.0.{{pollable as wasi-io-pollable}};"
    )?;

    if def.always_inline_types {
        let mut inline_types: Vec<InterfaceStubTypeDef> = vec![];

        for (import, type_def_info) in all_imports {
            match &type_def_info.package_name {
                Some(package) if package == &def.source_package_name => {
                    inline_types.push(type_def_info.clone());
                }
                _ => writeln!(out, "  use {}.{{{}}};", import.path, import.name)?,
            }
        }

        writeln!(out)?;

        for typ in inline_types {
            write_type_def(&mut out, &typ.type_def, typ.name.as_str(), def)?;
        }
    } else {
        for (import, _) in all_imports {
            writeln!(out, "  use {}.{{{}}};", import.path, import.name)?;
        }
        writeln!(out)?;
    }

    // Generating async return types
    for interface in def.source_interfaces() {
        for function in &interface.functions {
            if !function.results.is_empty() {
                write_async_return_type(&mut out, function, interface, def)?;
            }
        }
        for function in &interface.static_functions {
            if !function.results.is_empty() {
                write_async_return_type(&mut out, function, interface, def)?;
            }
        }
    }

    // Generating function definitions
    for interface in def.source_interfaces() {
        writeln!(out, "  resource {} {{", &interface.name)?;
        match &interface.constructor_params {
            None => {
                writeln!(out, "    constructor(location: golem-rpc-uri);")?;
            }
            Some(params) => {
                write!(out, "    constructor(location: golem-rpc-uri")?;
                if !params.is_empty() {
                    write!(out, ", ")?;
                }
                write_param_list(&mut out, def, params)?;
                writeln!(out, ");")?;
            }
        }
        for function in &interface.functions {
            write_function_definition(&mut out, function, false, interface, def)?;
        }
        for function in &interface.static_functions {
            write_function_definition(&mut out, function, true, interface, def)?;
        }
        writeln!(out, "  }}")?;
        writeln!(out)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    writeln!(out, "world {} {{", def.target_world_name())?;
    writeln!(out, "  export stub-{};", world.name)?;
    writeln!(out, "}}")?;

    Ok(out)
}

fn write_function_definition(
    out: &mut String,
    function: &FunctionStub,
    is_static: bool,
    owner: &InterfaceStub,
    def: &StubDefinition,
) -> anyhow::Result<()> {
    let func = if is_static { "static_func" } else { "func" };
    if !function.results.is_empty() {
        // Write the blocking function
        write!(out, "    blocking-{}: {func}(", function.name)?;
        write_param_list(out, def, &function.params)?;
        write!(out, ")")?;
        write!(out, " -> ")?;
        write_function_result_type(out, function, def)?;
        writeln!(out, ";")?;
        // Write the non-blocking function
        write!(out, "    {}: {func}(", function.name)?;
        write_param_list(out, def, &function.params)?;
        write!(out, ")")?;
        write!(out, " -> {}", function.async_result_type(owner))?;
    } else {
        // Write the blocking function
        write!(out, "    blocking-{}: {func}(", function.name)?;
        write_param_list(out, def, &function.params)?;
        write!(out, ")")?;
        writeln!(out, ";")?;

        // Write the non-blocking function
        write!(out, "    {}: {func}(", function.name)?;
        write_param_list(out, def, &function.params)?;
        write!(out, ")")?;
    }
    writeln!(out, ";")?;
    Ok(())
}

fn write_async_return_type(
    out: &mut String,
    function: &FunctionStub,
    owner: &InterfaceStub,
    def: &StubDefinition,
) -> anyhow::Result<()> {
    writeln!(out, "  resource {} {{", function.async_result_type(owner))?;
    writeln!(out, "    subscribe: func() -> wasi-io-pollable;")?;
    write!(out, "    get: func() -> option<")?;
    write_function_result_type(out, function, def)?;
    writeln!(out, ">;")?;
    writeln!(out, "  }}")?;
    Ok(())
}

fn write_function_result_type(
    out: &mut String,
    function: &FunctionStub,
    def: &StubDefinition,
) -> anyhow::Result<()> {
    match &function.results {
        FunctionResultStub::Single(typ) => {
            write!(out, "{}", typ.wit_type_string(def)?)?;
        }
        FunctionResultStub::Multi(params) => {
            write!(out, "(")?;
            write_param_list(out, def, params)?;
            write!(out, ")")?;
        }
        FunctionResultStub::SelfType => {
            return Err(anyhow!("Unexpected return type in wit generator"));
        }
    }
    Ok(())
}

fn write_type_def(
    out: &mut String,
    typ: &TypeDef,
    typ_name: &str,
    def: &StubDefinition,
) -> anyhow::Result<()> {
    let typ_kind = &typ.kind;
    let kind_str = typ_kind.as_str();

    match typ_kind {
        TypeDefKind::Record(record) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;

            write_record(out, &record.fields, def)?;
        }

        TypeDefKind::Flags(flags) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;

            write_flags(out, flags)?;
        }
        TypeDefKind::Tuple(tuple) => {
            if let Some(name) = &typ.name {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_tuple(out, tuple, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_tuple(out, tuple, def)?;
            }
        }
        TypeDefKind::Variant(variant) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;
            write_variant(out, variant, def)?;
        }
        TypeDefKind::Enum(enum_ty) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;
            write_enum(out, enum_ty)?;
        }
        TypeDefKind::Option(option) => {
            if let Some(name) = &typ.name {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_option(out, option, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_option(out, option, def)?;
            }
        }

        TypeDefKind::Result(result) => {
            if let Some(name) = &typ.name {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_result(out, result, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_result(out, result, def)?;
            }
        }

        TypeDefKind::List(list_typ) => {
            if let Some(name) = &typ.name {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write!(out, " {}", list_typ.wit_type_string(def)?)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write!(out, " {}", list_typ.wit_type_string(def)?)?;
            }
        }
        TypeDefKind::Future(_) => {}
        TypeDefKind::Stream(_) => {}
        TypeDefKind::Type(aliased_typ) => match aliased_typ {
            Type::Id(type_id) => {
                let aliased_type_def = def.get_type_def(*type_id)?;
                if aliased_type_def.owner == typ.owner {
                    // type alias to type defined in the same interface
                    write!(out, "  {} ", kind_str)?;
                    write!(out, "{}", typ_name)?;
                    write!(out, " =")?;
                    write!(out, " {}", aliased_typ.wit_type_string(def)?)?;
                    writeln!(out, ";")?;
                } else {
                    // import from another interface
                    write_type_def(out, aliased_type_def, typ_name, def)?;
                }
            }
            _ => {
                // type alias to primitive
                write!(out, "  {} ", kind_str)?;
                write!(out, "{}", typ_name)?;
                write!(out, " =")?;
                write!(out, " {}", aliased_typ.wit_type_string(def)?)?;
                writeln!(out, ";")?;
            }
        },
        TypeDefKind::Unknown => {
            write!(out, "  {}", kind_str)?;
        }
        TypeDefKind::Resource => {
            write!(out, "  {}", kind_str)?;
        }
        TypeDefKind::Handle(handle) => {
            write!(out, "  {}", kind_str)?;
            write_handle(out, handle, def)?;
        }
    }

    Ok(())
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#wit-types
fn write_handle(out: &mut String, handle: &Handle, def: &StubDefinition) -> anyhow::Result<()> {
    match handle {
        Handle::Own(type_id) => {
            write!(out, "{}", Type::Id(*type_id).wit_type_string(def)?)?;
        }
        Handle::Borrow(type_id) => {
            write!(out, " borrow<")?;
            write!(out, "{}", Type::Id(*type_id).wit_type_string(def)?)?;
            write!(out, ">")?;
        }
    }

    Ok(())
}

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md#wit-types
fn write_result(out: &mut String, result: &Result_, def: &StubDefinition) -> anyhow::Result<()> {
    match (result.ok, result.err) {
        (Some(ok), Some(err)) => {
            write!(out, "<")?;
            write!(out, "{}", ok.wit_type_string(def)?)?;
            write!(out, ", ")?;
            write!(out, "{}", err.wit_type_string(def)?)?;
            write!(out, ">")?;
        }
        (Some(ok), None) => {
            write!(out, "<")?;
            write!(out, "{}", ok.wit_type_string(def)?)?;
            write!(out, ">")?;
        }
        (None, Some(err)) => {
            write!(out, "<_, ")?;
            write!(out, "{}", err.wit_type_string(def)?)?;
            write!(out, ">")?;
        }
        (None, None) => {}
    }

    Ok(())
}

fn write_option(out: &mut String, option: &Type, def: &StubDefinition) -> anyhow::Result<()> {
    write!(out, "<")?;
    write!(out, "{}", option.wit_type_string(def)?)?;
    write!(out, ">")?;
    Ok(())
}

fn write_variant(out: &mut String, variant: &Variant, def: &StubDefinition) -> anyhow::Result<()> {
    let cases_length = variant.cases.len();
    write!(out, " {{")?;
    writeln!(out)?;

    for (idx, case) in variant.cases.iter().enumerate() {
        write!(out, "    {}", case.name)?;

        if let Some(ty) = case.ty {
            write!(out, "(")?;
            write!(out, "{}", ty.wit_type_string(def)?)?;
            write!(out, ")")?;
        }

        if idx < cases_length - 1 {
            writeln!(out, ", ")?;
        }
    }

    writeln!(out, "  }}")?;
    Ok(())
}

fn write_enum(out: &mut String, enum_ty: &Enum) -> anyhow::Result<()> {
    let length = enum_ty.cases.len();
    write!(out, " {{")?;
    writeln!(out)?;
    for (idx, case) in enum_ty.cases.iter().enumerate() {
        write!(out, "    {}", case.name)?;
        if idx < length - 1 {
            writeln!(out, ",")?;
        }
        writeln!(out)?;
    }
    writeln!(out, "  }}")?;
    Ok(())
}

fn write_tuple(out: &mut String, tuple: &Tuple, def: &StubDefinition) -> anyhow::Result<()> {
    let tuple_length = tuple.types.len();
    write!(out, " <")?;
    for (idx, typ) in tuple.types.iter().enumerate() {
        write!(out, "{}", typ.wit_type_string(def)?)?;
        if idx < tuple_length - 1 {
            write!(out, ", ")?;
        }
    }
    write!(out, "  >")?;
    Ok(())
}

fn write_record(out: &mut String, fields: &[Field], def: &StubDefinition) -> anyhow::Result<()> {
    write!(out, " {{")?;
    writeln!(out)?;

    for (idx, field) in fields.iter().enumerate() {
        write!(
            out,
            "    {}: {}",
            field.name,
            field.ty.wit_type_string(def)?
        )?;
        if idx < fields.len() - 1 {
            write!(out, ", ")?;
        }
    }

    writeln!(out)?;
    writeln!(out, "  }}")?;

    Ok(())
}

fn write_flags(out: &mut String, flags: &Flags) -> anyhow::Result<()> {
    write!(out, " {{")?;
    writeln!(out)?;
    let flags_len = flags.flags.len();
    for (idx, flag) in flags.flags.iter().enumerate() {
        write!(out, "    {}", flag.name)?;
        if idx < flags_len - 1 {
            writeln!(out, ",")?;
        }
        writeln!(out)?;
    }
    writeln!(out, "  }}")?;
    Ok(())
}

fn write_param_list(
    out: &mut String,
    def: &StubDefinition,
    params: &[FunctionParamStub],
) -> anyhow::Result<()> {
    for (idx, param) in params.iter().enumerate() {
        write!(out, "{}: {}", param.name, param.typ.wit_type_string(def)?)?;
        if idx < params.len() - 1 {
            write!(out, ", ")?;
        }
    }
    Ok(())
}

pub fn copy_wit_files(def: &StubDefinition) -> anyhow::Result<()> {
    let stub_package_name = def.stub_package_name();
    let remove_stub_imports = import_remover(&stub_package_name);

    let target_wit_root = def.target_wit_root();
    let target_deps = target_wit_root.join(naming::wit::DEPS_DIR);

    for (package, sources) in def.source_packages_with_wit_sources() {
        if package.name == stub_package_name {
            log_warn_action("Skipping", format!("package {}", package.name));
            continue;
        }

        let is_source_package = package.name == def.source_package_name;

        log_action("Copying", format!("source package {}", package.name));
        for source in sources {
            if is_source_package {
                // TODO: naming to def and review this naming
                let dest = target_deps
                    .join(format!(
                        "{}_{}",
                        def.source_package_name.namespace, def.source_package_name.name
                    ))
                    .join(source.file_name().ok_or_else(|| {
                        anyhow!(
                            "Failed to get file name for source: {}",
                            source.to_string_lossy()
                        )
                    })?);
                log_action(
                    "  Copying",
                    format!(
                        "(with source imports removed) {} to {}",
                        source.to_string_lossy(),
                        dest.to_string_lossy()
                    ),
                );
                copy_transformed(source, &dest, &remove_stub_imports)?;
            } else {
                let relative = source.strip_prefix(&def.source_wit_root)?;
                let dest = target_wit_root.join(relative);
                log_action(
                    "  Copying",
                    format!("{} to {}", source.to_string_lossy(), dest.to_string_lossy()),
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
        format!("{} to {}", file_name, target_dir.to_string_lossy()),
    );

    fs::write(target_dir.join(file_name), content)?;

    Ok(())
}

trait TypeExtensions {
    fn wit_type_string(&self, stub_definition: &StubDefinition) -> anyhow::Result<String>;
}

impl TypeExtensions for Type {
    fn wit_type_string(&self, stub_definition: &StubDefinition) -> anyhow::Result<String> {
        match self {
            Type::Bool => Ok("bool".to_string()),
            Type::U8 => Ok("u8".to_string()),
            Type::U16 => Ok("u16".to_string()),
            Type::U32 => Ok("u32".to_string()),
            Type::U64 => Ok("u64".to_string()),
            Type::S8 => Ok("s8".to_string()),
            Type::S16 => Ok("s16".to_string()),
            Type::S32 => Ok("s32".to_string()),
            Type::S64 => Ok("s64".to_string()),
            Type::F32 => Ok("f32".to_string()),
            Type::F64 => Ok("f64".to_string()),
            Type::Char => Ok("char".to_string()),
            Type::String => Ok("string".to_string()),
            Type::Id(type_id) => {
                let typ = stub_definition.get_type_def(*type_id)?;
                match &typ.kind {
                    TypeDefKind::Option(inner) => Ok(format!(
                        "option<{}>",
                        inner.wit_type_string(stub_definition)?
                    )),
                    TypeDefKind::List(inner) => {
                        Ok(format!("list<{}>", inner.wit_type_string(stub_definition)?))
                    }
                    TypeDefKind::Tuple(tuple) => {
                        let types = tuple
                            .types
                            .iter()
                            .map(|t| t.wit_type_string(stub_definition))
                            .collect::<anyhow::Result<Vec<_>>>()?;
                        Ok(format!("tuple<{}>", types.join(", ")))
                    }
                    TypeDefKind::Result(result) => match (&result.ok, &result.err) {
                        (Some(ok), Some(err)) => {
                            let ok = ok.wit_type_string(stub_definition)?;
                            let err = err.wit_type_string(stub_definition)?;
                            Ok(format!("result<{}, {}>", ok, err))
                        }
                        (Some(ok), None) => {
                            let ok = ok.wit_type_string(stub_definition)?;
                            Ok(format!("result<{}>", ok))
                        }
                        (None, Some(err)) => {
                            let err = err.wit_type_string(stub_definition)?;
                            Ok(format!("result<_, {}>", err))
                        }
                        (None, None) => {
                            bail!("result type has no ok or err types")
                        }
                    },
                    TypeDefKind::Handle(handle) => match handle {
                        Handle::Own(type_id) => Type::Id(*type_id).wit_type_string(stub_definition),
                        Handle::Borrow(type_id) => Ok(format!(
                            "borrow<{}>",
                            Type::Id(*type_id).wit_type_string(stub_definition)?
                        )),
                    },
                    _ => {
                        let name = typ
                            .name
                            .clone()
                            .ok_or(anyhow!("wit_type_string: type has no name"))?;
                        Ok(name)
                    }
                }
            }
        }
    }
}

pub fn import_remover(package_name: &PackageName) -> impl Fn(String) -> anyhow::Result<String> {
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
