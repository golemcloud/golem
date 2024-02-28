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

use crate::stub::{FunctionParamStub, FunctionResultStub, StubDefinition};
use anyhow::{anyhow, bail, Context};
use indexmap::IndexSet;
use std::fmt::{Display, Formatter, Write};
use std::fs;
use std::path::{Path, PathBuf};
use wit_parser::{Handle, PackageName, Resolve, Type, TypeDefKind, UnresolvedPackage};

pub fn generate_stub_wit(def: &StubDefinition) -> anyhow::Result<()> {
    let world = def.resolve.worlds.get(def.world_id).unwrap();

    let mut out = String::new();

    writeln!(out, "package {}-stub;", def.root_package_name)?;
    writeln!(out)?;
    writeln!(out, "interface stub-{} {{", world.name)?;

    let all_imports = def
        .interfaces
        .iter()
        .flat_map(|i| i.imports.iter())
        .collect::<IndexSet<_>>();

    writeln!(out, "  use golem:rpc/types@0.1.0.{{uri}};")?;
    for import in all_imports {
        writeln!(out, "  use {}.{{{}}};", import.path, import.name)?;
    }
    writeln!(out)?;

    for interface in &def.interfaces {
        writeln!(out, "  resource {} {{", &interface.name)?;
        match &interface.constructor_params {
            None => {
                writeln!(out, "    constructor(location: uri);")?;
            }
            Some(params) => {
                write!(out, "    constructor(location: uri")?;
                if !params.is_empty() {
                    write!(out, ", ")?;
                }
                write_param_list(&mut out, def, params)?;
                writeln!(out, ");")?;
            }
        }
        for function in &interface.functions {
            write!(out, "    {}: func(", function.name)?;
            write_param_list(&mut out, def, &function.params)?;
            write!(out, ")")?;
            if !function.results.is_empty() {
                write!(out, " -> ")?;
                match &function.results {
                    FunctionResultStub::Single(typ) => {
                        write!(out, "{}", typ.wit_type_string(&def.resolve)?)?;
                    }
                    FunctionResultStub::Multi(params) => {
                        write!(out, "(")?;
                        write_param_list(&mut out, def, params)?;
                        write!(out, ")")?;
                    }
                    FunctionResultStub::SelfType => {
                        return Err(anyhow!("Unexpected return type in wit generator"));
                    }
                }
            }
            writeln!(out, ";")?;
        }
        for function in &interface.static_functions {
            write!(out, "    {}: static func(", function.name)?;
            write_param_list(&mut out, def, &function.params)?;
            write!(out, ")")?;
            if !function.results.is_empty() {
                write!(out, " -> ")?;
                match &function.results {
                    FunctionResultStub::Single(typ) => {
                        write!(out, "{}", typ.wit_type_string(&def.resolve)?)?;
                    }
                    FunctionResultStub::Multi(params) => {
                        write!(out, "(")?;
                        write_param_list(&mut out, def, params)?;
                        write!(out, ")")?;
                    }
                    FunctionResultStub::SelfType => {
                        return Err(anyhow!("Unexpected return type in wit generator"));
                    }
                }
            }
            writeln!(out, ";")?;
        }
        writeln!(out, "  }}")?;
        writeln!(out)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

    writeln!(out, "world {} {{", def.target_world_name()?)?;
    writeln!(out, "  export stub-{};", world.name)?;
    writeln!(out, "}}")?;

    println!(
        "Generating stub WIT to {}",
        def.target_wit_path().to_string_lossy()
    );
    fs::create_dir_all(def.target_wit_root())?;
    fs::write(def.target_wit_path(), out)?;
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
                let dest = dep_dir.join(source.file_name().unwrap());
                println!(
                    "  .. {} to {}",
                    source.to_string_lossy(),
                    dest.to_string_lossy()
                );

                fs::create_dir_all(dest.parent().unwrap())?;
                fs::copy(source, &dest)?;
            }
        } else {
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
            Type::Float32 => Ok("f32".to_string()),
            Type::Float64 => Ok("f64".to_string()),
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

pub enum WitAction {
    CopyDepDir {
        source_dir: PathBuf,
    },
    CopyDepWit {
        source_wit: PathBuf,
        dir_name: String,
    },
}

impl Display for WitAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
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
