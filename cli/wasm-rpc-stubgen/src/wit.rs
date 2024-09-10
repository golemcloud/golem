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

use crate::stub::{
    FunctionParamStub, FunctionResultStub, FunctionStub, InterfaceStub, InterfaceStubImport,
    InterfaceStubTypeDef, StubDefinition,
};
use anyhow::{anyhow, bail, Context};
use indexmap::IndexMap;
use regex::Regex;
use std::ffi::OsStr;
use std::fmt::{Display, Formatter, Write};
use std::fs;
use std::path::{Path, PathBuf};
use wit_parser::{
    Enum, Field, Flags, Handle, PackageName, Resolve, Result_, Tuple, Type, TypeDef, TypeDefKind,
    UnresolvedPackage, Variant,
};

pub fn generate_stub_wit(def: &StubDefinition) -> anyhow::Result<()> {
    let type_gen_strategy = if def.always_inline_types {
        StubTypeGen::InlineRootTypes
    } else {
        StubTypeGen::ImportRootTypes
    };

    let out = get_stub_wit(def, type_gen_strategy)?;
    println!(
        "Generating stub WIT to {}",
        def.target_wit_path().to_string_lossy()
    );
    fs::create_dir_all(def.target_wit_root())?;
    fs::write(def.target_wit_path(), out)?;
    Ok(())
}

pub enum StubTypeGen {
    InlineRootTypes,
    ImportRootTypes,
}

pub fn get_stub_wit(
    def: &StubDefinition,
    type_gen_strategy: StubTypeGen,
) -> anyhow::Result<String> {
    let world = def.resolve.worlds.get(def.world_id).unwrap();

    let mut out = String::new();

    writeln!(out, "package {}-stub;", def.root_package_name)?;
    writeln!(out)?;
    writeln!(out, "interface stub-{} {{", world.name)?;

    let all_imports = def
        .interfaces
        .iter()
        .flat_map(|i| i.imports.iter().map(|i| (InterfaceStubImport::from(i), i)))
        .collect::<IndexMap<_, _>>();

    // Renaming the mandatory imports to avoid collisions with types coming from the stubbed package
    writeln!(out, "  use golem:rpc/types@0.1.0.{{uri as golem-rpc-uri}};")?;
    writeln!(
        out,
        "  use wasi:io/poll@0.2.0.{{pollable as wasi-io-pollable}};"
    )?;

    match type_gen_strategy {
        StubTypeGen::ImportRootTypes => {
            for (import, _) in all_imports {
                writeln!(out, "  use {}.{{{}}};", import.path, import.name)?;
            }
            writeln!(out)?;
        }
        StubTypeGen::InlineRootTypes => {
            let mut inline_types: Vec<InterfaceStubTypeDef> = vec![];

            for (import, type_def_info) in all_imports {
                match &type_def_info.package_name {
                    Some(package) if package == &def.root_package_name => {
                        inline_types.push(type_def_info.clone());
                    }
                    _ => writeln!(out, "  use {}.{{{}}};", import.path, import.name)?,
                }
            }

            writeln!(out)?;

            for typ in inline_types {
                write_type_def(&mut out, &typ.type_def, typ.name.as_str(), def)?;
            }
        }
    }

    // Generating async return types
    for interface in &def.interfaces {
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
    for interface in &def.interfaces {
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

    writeln!(out, "world {} {{", def.target_world_name()?)?;
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
            write!(out, "{}", typ.wit_type_string(&def.resolve)?)?;
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
    let typ_kind = typ.clone().kind;
    let kind_str = typ_kind.as_str();

    match typ_kind {
        TypeDefKind::Record(record) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;

            write_record(out, record.fields, def)?;
        }

        TypeDefKind::Flags(flags) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;

            write_flags(out, &flags)?;
        }
        TypeDefKind::Tuple(tuple) => {
            if let Some(name) = typ.name.clone() {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_tuple(out, &tuple, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_tuple(out, &tuple, def)?;
            }
        }
        TypeDefKind::Variant(variant) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;
            write_variant(out, &variant, def)?;
        }
        TypeDefKind::Enum(enum_ty) => {
            write!(out, "  {}", kind_str)?;
            write!(out, " {}", typ_name)?;
            write_enum(out, enum_ty)?;
        }
        TypeDefKind::Option(option) => {
            if let Some(name) = typ.name.clone() {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_option(out, &option, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_option(out, &option, def)?;
            }
        }

        TypeDefKind::Result(result) => {
            if let Some(name) = typ.name.clone() {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write_result(out, &result, def)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write_result(out, &result, def)?;
            }
        }

        TypeDefKind::List(list_typ) => {
            if let Some(name) = typ.name.clone() {
                write!(out, "  type {} =", name)?;
                write!(out, "  {}", kind_str)?;
                write!(out, " {}", list_typ.wit_type_string(&def.resolve)?)?;
                writeln!(out, ";")?;
            } else {
                write!(out, "  {}", kind_str)?;
                write!(out, " {}", list_typ.wit_type_string(&def.resolve)?)?;
            }
        }
        TypeDefKind::Future(_) => {}
        TypeDefKind::Stream(_) => {}
        TypeDefKind::Type(typ) => {
            write!(out, "  {} ", kind_str)?;
            write!(out, "{}", typ_name)?;
            write!(out, " =")?;
            write!(out, " {}", typ.wit_type_string(&def.resolve)?)?;
            writeln!(out, ";")?;
        }
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
fn write_handle(out: &mut String, handle: Handle, def: &StubDefinition) -> anyhow::Result<()> {
    match handle {
        Handle::Own(type_id) => {
            write!(out, "{}", Type::Id(type_id).wit_type_string(&def.resolve)?)?;
        }
        Handle::Borrow(type_id) => {
            write!(out, " borrow<")?;
            write!(out, "{}", Type::Id(type_id).wit_type_string(&def.resolve)?)?;
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
            write!(out, "{}", ok.wit_type_string(&def.resolve)?)?;
            write!(out, ", ")?;
            write!(out, "{}", err.wit_type_string(&def.resolve)?)?;
            write!(out, ">")?;
        }
        (Some(ok), None) => {
            write!(out, "<")?;
            write!(out, "{}", ok.wit_type_string(&def.resolve)?)?;
            write!(out, ">")?;
        }
        (None, Some(err)) => {
            write!(out, "<_, ")?;
            write!(out, "{}", err.wit_type_string(&def.resolve)?)?;
            write!(out, ">")?;
        }
        (None, None) => {}
    }

    Ok(())
}

fn write_option(out: &mut String, option: &Type, def: &StubDefinition) -> anyhow::Result<()> {
    write!(out, "<")?;
    write!(out, "{}", option.wit_type_string(&def.resolve)?)?;
    write!(out, ">")?;
    Ok(())
}

fn write_variant(out: &mut String, variant: &Variant, def: &StubDefinition) -> anyhow::Result<()> {
    let cases_length = variant.cases.len();
    write!(out, " {{")?;
    writeln!(out)?;

    for (idx, case) in variant.cases.iter().enumerate() {
        let case_name = case.name.clone();
        write!(out, "    {}", case_name)?;

        if let Some(ty) = case.ty {
            write!(out, "(")?;
            write!(out, "{}", ty.wit_type_string(&def.resolve)?)?;
            write!(out, ")")?;
        }

        if idx < cases_length - 1 {
            writeln!(out, ", ")?;
        }
    }

    writeln!(out, "  }}")?;
    Ok(())
}

fn write_enum(out: &mut String, enum_ty: Enum) -> anyhow::Result<()> {
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
        write!(out, "{}", typ.wit_type_string(&def.resolve)?)?;
        if idx < tuple_length - 1 {
            write!(out, ", ")?;
        }
    }
    write!(out, "  >")?;
    Ok(())
}

fn write_record(out: &mut String, fields: Vec<Field>, def: &StubDefinition) -> anyhow::Result<()> {
    write!(out, " {{")?;
    writeln!(out)?;

    for (idx, field) in fields.iter().enumerate() {
        write!(
            out,
            "    {}: {}",
            field.name,
            field.ty.wit_type_string(&def.resolve)?
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
        write!(
            out,
            "{}: {}",
            param.name,
            param.typ.wit_type_string(&def.resolve)?
        )?;
        if idx < params.len() - 1 {
            write!(out, ", ")?;
        }
    }
    Ok(())
}

pub fn copy_wit_files(def: &StubDefinition) -> anyhow::Result<()> {
    let mut all = def.unresolved_deps.clone();
    all.push(def.unresolved_root.clone());
    let stub_package_name = format!("{}-stub", def.root_package_name);

    let dest_wit_root = def.target_wit_root();
    fs::create_dir_all(&dest_wit_root)?;

    for unresolved in all {
        if unresolved.name == def.root_package_name {
            println!("Copying root package {}", unresolved.name);
            let dep_dir = dest_wit_root
                .clone()
                .join(Path::new("deps"))
                .join(Path::new(&format!(
                    "{}_{}",
                    def.root_package_name.namespace, def.root_package_name.name
                )));

            fs::create_dir_all(&dep_dir)?;
            for source in unresolved.source_files() {
                // While copying this root WIT (from which the stub is generated) to the deps directory
                // of the stub, we ensure there is no import of the same stub's types. This happens
                // when you regenerate a stub from a WIT, which the user may have manually modified to refer to the same stub.
                // This occurs only when there is a cyclic dependency. That's when the package from which the stub is generated is using the stub itself.
                //
                // Replacing this import with `none` is closer to being the right thing to do at this stage,
                // because there is no reason a stub WIT needs to cyclically depend on its own types to generate the stub.
                // Now the question is whether we need to parse this WIT to Resolved and then have our own Display
                // for it, or just use regex replace. The latter is chosen because we hardly refer to the same stub without
                // `import`, and the actual stub package name.
                // The former may look cleaner, but slower, and may not bring about anymore significant correctness or reliability.
                let read_data = fs::read_to_string(source)?;
                let re = Regex::new(
                    format!(
                        r"import\s+{}(/[^;]*)?;",
                        regex::escape(stub_package_name.as_str())
                    )
                    .as_str(),
                )
                .unwrap();
                let new_data = re.replace_all(&read_data, "");

                let dest = dep_dir.join(source.file_name().unwrap());
                println!(
                    "  .. {} to {}",
                    source.to_string_lossy(),
                    dest.to_string_lossy()
                );

                fs::create_dir_all(dest.parent().unwrap())?;
                fs::write(&dest, new_data.to_string())?;
            }
        } else if unresolved.name.to_string() != stub_package_name {
            println!("Copying package {}", unresolved.name);

            for source in unresolved.source_files() {
                let relative = source.strip_prefix(&def.source_wit_root)?;
                let dest = dest_wit_root.clone().join(relative);
                println!(
                    "  .. {} to {}",
                    source.to_string_lossy(),
                    dest.to_string_lossy()
                );
                fs::create_dir_all(dest.parent().unwrap())?;
                fs::copy(source, &dest)?;
            }
        } else {
            println!("Skipping copying stub package {}", stub_package_name);
        }
    }
    let wasm_rpc_root = dest_wit_root.join(Path::new("deps/wasm-rpc"));
    fs::create_dir_all(&wasm_rpc_root).unwrap();

    println!(
        "Writing wasm-rpc.wit to {}",
        wasm_rpc_root.to_string_lossy()
    );
    fs::write(
        wasm_rpc_root.join(Path::new("wasm-rpc.wit")),
        golem_wasm_rpc::WASM_RPC_WIT,
    )?;

    let wasi_poll_root = dest_wit_root.join(Path::new("deps/io"));
    fs::create_dir_all(&wasi_poll_root).unwrap();
    println!("Writing poll.wit to {}", wasi_poll_root.to_string_lossy());
    fs::write(
        wasi_poll_root.join(Path::new("poll.wit")),
        golem_wasm_rpc::WASI_POLL_WIT,
    )?;

    Ok(())
}

trait TypeExtensions {
    fn wit_type_string(&self, resolve: &Resolve) -> anyhow::Result<String>;
}

impl TypeExtensions for Type {
    fn wit_type_string(&self, resolve: &Resolve) -> anyhow::Result<String> {
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
                let typ = resolve
                    .types
                    .get(*type_id)
                    .ok_or(anyhow!("type not found"))?;

                match &typ.kind {
                    TypeDefKind::Option(inner) => {
                        Ok(format!("option<{}>", inner.wit_type_string(resolve)?))
                    }
                    TypeDefKind::List(inner) => {
                        Ok(format!("list<{}>", inner.wit_type_string(resolve)?))
                    }
                    TypeDefKind::Tuple(tuple) => {
                        let types = tuple
                            .types
                            .iter()
                            .map(|t| t.wit_type_string(resolve))
                            .collect::<anyhow::Result<Vec<_>>>()?;
                        Ok(format!("tuple<{}>", types.join(", ")))
                    }
                    TypeDefKind::Result(result) => match (&result.ok, &result.err) {
                        (Some(ok), Some(err)) => {
                            let ok = ok.wit_type_string(resolve)?;
                            let err = err.wit_type_string(resolve)?;
                            Ok(format!("result<{}, {}>", ok, err))
                        }
                        (Some(ok), None) => {
                            let ok = ok.wit_type_string(resolve)?;
                            Ok(format!("result<{}>", ok))
                        }
                        (None, Some(err)) => {
                            let err = err.wit_type_string(resolve)?;
                            Ok(format!("result<_, {}>", err))
                        }
                        (None, None) => {
                            bail!("result type has no ok or err types")
                        }
                    },
                    TypeDefKind::Handle(handle) => match handle {
                        Handle::Own(type_id) => Type::Id(*type_id).wit_type_string(resolve),
                        Handle::Borrow(type_id) => Ok(format!(
                            "borrow<{}>",
                            Type::Id(*type_id).wit_type_string(resolve)?
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

pub fn get_dep_dirs(wit_root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let deps = wit_root.join("deps");
    if deps.exists() && deps.is_dir() {
        for entry in fs::read_dir(deps)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                result.push(entry.path());
            }
        }
    }
    Ok(result)
}

pub fn get_package_name(wit: &Path) -> anyhow::Result<PackageName> {
    let pkg = UnresolvedPackage::parse_path(wit)?;
    Ok(pkg.name)
}

#[allow(clippy::enum_variant_names)]
pub enum WitAction {
    CopyDepDir {
        source_dir: PathBuf,
    },
    WriteWit {
        source_wit: String,
        dir_name: String,
        file_name: String,
    },
    CopyDepWit {
        source_wit: PathBuf,
        dir_name: String,
    },
}

impl Display for WitAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WitAction::WriteWit {
                dir_name,
                file_name,
                ..
            } => {
                write!(
                    f,
                    "copy stub WIT string to relative path {}/{}",
                    dir_name, file_name
                )
            }
            WitAction::CopyDepDir { source_dir } => {
                write!(
                    f,
                    "copy WIT dependency from {}",
                    source_dir.to_string_lossy()
                )
            }
            WitAction::CopyDepWit {
                source_wit,
                dir_name,
            } => {
                write!(
                    f,
                    "copy stub WIT from {} as dependency {}",
                    source_wit.to_string_lossy(),
                    dir_name
                )
            }
        }
    }
}

impl WitAction {
    pub fn perform(&self, target_wit_root: &Path) -> anyhow::Result<()> {
        match self {
            WitAction::WriteWit {
                source_wit,
                dir_name,
                file_name,
            } => {
                let target_dir = target_wit_root.join("deps").join(dir_name);
                if !target_dir.exists() {
                    fs::create_dir_all(&target_dir).context("Create target directory")?;
                }
                fs::write(target_dir.join(OsStr::new(file_name)), source_wit)
                    .context("Copy the WIT string")?;
            }

            WitAction::CopyDepDir { source_dir } => {
                let dep_name = source_dir
                    .file_name()
                    .context("Get wit dependency directory name")?;
                let target_path = target_wit_root.join("deps").join(dep_name);
                if !target_path.exists() {
                    fs::create_dir_all(&target_path).context("Create target directory")?;
                }
                println!("Copying {source_dir:?} to {target_path:?}");
                fs_extra::dir::copy(
                    source_dir,
                    &target_path,
                    &fs_extra::dir::CopyOptions::new()
                        .content_only(true)
                        .overwrite(true),
                )
                .context("Failed to copy the dependency directory")?;
            }
            WitAction::CopyDepWit {
                source_wit,
                dir_name,
            } => {
                let target_dir = target_wit_root.join("deps").join(dir_name);
                if !target_dir.exists() {
                    fs::create_dir_all(&target_dir).context("Create target directory")?;
                }
                fs::copy(source_wit, target_dir.join(source_wit.file_name().unwrap()))
                    .context("Copy the WIT file")?;
            }
        }

        Ok(())
    }

    pub fn get_dep_dir_name(&self) -> anyhow::Result<String> {
        match self {
            WitAction::CopyDepDir { source_dir } => Ok(source_dir
                .file_name()
                .context("Get wit dependency directory name")?
                .to_string_lossy()
                .to_string()),
            WitAction::WriteWit { dir_name, .. } => Ok(dir_name.clone()),
            WitAction::CopyDepWit { dir_name, .. } => Ok(dir_name.clone()),
        }
    }
}

pub fn verify_action(
    action: &WitAction,
    target_wit_root: &Path,
    overwrite: bool,
) -> anyhow::Result<bool> {
    match action {
        WitAction::WriteWit {
            source_wit,
            dir_name,
            file_name,
        } => {
            let target_dir = target_wit_root.join("deps").join(dir_name);
            let target_wit = target_dir.join(file_name);
            if target_dir.exists() && target_dir.is_dir() {
                let target_contents = fs::read_to_string(&target_wit)?;
                if source_wit == &target_contents {
                    Ok(true)
                } else if overwrite {
                    println!("Overwriting {}", target_wit.to_string_lossy());
                    Ok(true)
                } else {
                    Ok(false)
                }
            } else {
                Ok(true)
            }
        }

        WitAction::CopyDepDir { source_dir } => {
            let dep_name = source_dir
                .file_name()
                .context("Get wit dependency directory name")?;
            let target_path = target_wit_root.join("deps").join(dep_name);
            if target_path.exists() && target_path.is_dir() {
                if !dir_diff::is_different(source_dir, &target_path)? {
                    Ok(true)
                } else if overwrite {
                    println!("Overwriting {}", target_path.to_string_lossy());
                    Ok(true)
                } else {
                    Ok(false)
                }
            } else {
                Ok(true)
            }
        }
        WitAction::CopyDepWit {
            source_wit,
            dir_name,
        } => {
            let target_dir = target_wit_root.join("deps").join(dir_name);
            let source_file_name = source_wit.file_name().context("Get source wit file name")?;
            let target_wit = target_dir.join(source_file_name);
            if target_dir.exists() && target_dir.is_dir() {
                let mut existing_entries = Vec::new();
                for entry in fs::read_dir(&target_dir)? {
                    let entry = entry?;
                    let name = entry
                        .path()
                        .file_name()
                        .context("Get existing wit directory's name")?
                        .to_string_lossy()
                        .to_string();
                    existing_entries.push(name);
                }
                if existing_entries.contains(&source_file_name.to_string_lossy().to_string()) {
                    let source_contents = fs::read_to_string(source_wit)?;
                    let target_contents = fs::read_to_string(&target_wit)?;
                    if source_contents == target_contents {
                        Ok(true)
                    } else if overwrite {
                        println!("Overwriting {}", target_wit.to_string_lossy());
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                } else {
                    Ok(true)
                }
            } else {
                Ok(true)
            }
        }
    }
}
