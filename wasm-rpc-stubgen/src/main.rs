use anyhow::{anyhow, bail};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting,
};
use heck::ToUpperCamelCase;
use id_arena::Id;
use indexmap::IndexSet;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::fs;
use std::path::Path;
use toml::Value;
use wit_bindgen_rust::to_rust_ident;
use wit_parser::*;

// https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md

fn visit<'a>(
    pkg: &'a UnresolvedPackage,
    deps: &'a BTreeMap<PackageName, UnresolvedPackage>,
    order: &mut IndexSet<PackageName>,
    visiting: &mut HashSet<&'a PackageName>,
) -> anyhow::Result<()> {
    if order.contains(&pkg.name) {
        return Ok(());
    }
    for (dep, _) in pkg.foreign_deps.iter() {
        if !visiting.insert(dep) {
            bail!("package depends on itself");
        }
        let dep = deps
            .get(dep)
            .ok_or_else(|| anyhow!("failed to find package `{dep}` in `deps` directory"))?;
        visit(dep, deps, order, visiting)?;
        assert!(visiting.remove(&dep.name));
    }
    assert!(order.insert(pkg.name.clone()));
    Ok(())
}

// Copied and modified from `wit-parser` crate
fn get_unresolved_packages(
    root_path: &Path,
) -> anyhow::Result<(UnresolvedPackage, Vec<UnresolvedPackage>)> {
    let root = UnresolvedPackage::parse_dir(root_path).unwrap();

    let mut deps = BTreeMap::new();
    let deps_path = root_path.join(Path::new("deps"));
    for dep_entry in fs::read_dir(deps_path).unwrap() {
        let dep_entry = dep_entry.unwrap();
        let dep = UnresolvedPackage::parse_path(&dep_entry.path()).unwrap();
        for src in dep.source_files() {
            println!("dep {dep_entry:?} source: {src:?}");
        }
        deps.insert(dep.name.clone(), dep);
    }

    // Perform a simple topological sort which will bail out on cycles
    // and otherwise determine the order that packages must be added to
    // this `Resolve`.
    let mut order = IndexSet::new();
    let mut visiting = HashSet::new();
    for pkg in deps.values().chain([&root]) {
        visit(&pkg, &deps, &mut order, &mut visiting)?;
    }

    let mut ordered_deps = Vec::new();
    for name in order {
        if let Some(pkg) = deps.remove(&name) {
            ordered_deps.push(pkg);
        }
    }

    Ok((root, ordered_deps))
}

fn main() {
    // TODO: inputs
    let root_path = Path::new("wasm-rpc-stubgen/example");
    let dest_root = Path::new("tmp/stubgen_out");
    let selected_world = Some("api");
    let stub_crate_version = "0.0.1".to_string();
    // ^^^

    let (root, deps) = get_unresolved_packages(root_path).unwrap();
    let root_package = root.name.clone();

    let mut resolve = Resolve::new();
    for unresolved in deps.iter().cloned() {
        resolve.push(unresolved).unwrap();
    }
    let root_id = resolve.push(root.clone()).unwrap();

    dump(&resolve);

    let world = resolve.select_world(root_id, selected_world).unwrap();
    let selected_world = resolve.worlds.get(world).unwrap().name.clone();

    let dest_wit_root = dest_root.join(Path::new("wit"));
    fs::create_dir_all(dest_root).unwrap();

    let stub_world_name = format!("wasm-rpc-stub-{}", selected_world);

    generate_stub_wit(
        &resolve,
        root_package.clone(),
        world,
        stub_world_name.clone(),
        &dest_wit_root.join(Path::new("_stub.wit")),
    )
    .unwrap();

    let mut all = deps.clone();
    all.push(root);
    for unresolved in all {
        println!("copying {:?}", unresolved.name);
        for source in unresolved.source_files() {
            let relative = source.strip_prefix(root_path).unwrap();
            let dest = dest_wit_root.join(relative);
            println!("Copying {source:?} to {dest:?}");
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::copy(&source, &dest).unwrap();
        }
    }
    let wasm_rpc_wit = include_str!("../../wasm-rpc/wit/wasm-rpc.wit");
    let wasm_rpc_root = dest_wit_root.join(Path::new("deps/wasm-rpc"));
    fs::create_dir_all(&wasm_rpc_root).unwrap();
    fs::write(wasm_rpc_root.join(Path::new("wasm-rpc.wit")), wasm_rpc_wit).unwrap();

    println!("----");

    let mut final_resolve = Resolve::new();
    final_resolve.push_dir(&dest_wit_root).unwrap();
    dump(&final_resolve);

    println!("generating cargo.toml");
    generate_cargo_toml(
        &root_path,
        &dest_root.join("Cargo.toml"),
        selected_world,
        stub_crate_version,
        format!("{}:{}", root_package.namespace, root_package.name),
        stub_world_name,
        &deps,
    )
    .unwrap();

    let dest_src_root = dest_root.join(Path::new("src"));
    fs::create_dir_all(&dest_src_root).unwrap();
    generate_stub_source(
        &dest_root.join("src/lib.rs"),
        &resolve,
        &world,
        root_package,
    )
    .unwrap();
}

#[derive(Serialize)]
struct MetadataRoot {
    component: Option<ComponentMetadata>,
}

impl Default for MetadataRoot {
    fn default() -> Self {
        MetadataRoot { component: None }
    }
}

#[derive(Serialize)]
struct ComponentMetadata {
    package: String,
    target: ComponentTarget,
}

#[derive(Serialize)]
struct ComponentTarget {
    world: String,
    path: String,
    dependencies: HashMap<String, WitDependency>,
}

#[derive(Serialize)]
struct WitDependency {
    path: String,
}

fn generate_cargo_toml(
    root_path: &Path,
    target: &Path,
    name: String,
    version: String,
    package: String,
    stub_world_name: String,
    deps: &[UnresolvedPackage],
) -> anyhow::Result<()> {
    let mut manifest = Manifest::default();

    let mut wit_dependencies = HashMap::new();
    for dep in deps {
        let mut dirs = HashSet::new();
        for source in dep.source_files() {
            let relative = source.strip_prefix(root_path)?;
            let dir = relative
                .parent()
                .ok_or(anyhow!("Package source {source:?} has no parent directory"))?;
            dirs.insert(dir);
        }

        if dirs.len() != 1 {
            bail!("Package {} has multiple source directories", dep.name);
        }

        wit_dependencies.insert(
            "golem:rpc".to_string(),
            WitDependency {
                path: "wit/deps/wasm-rpc".to_string(),
            },
        );
        wit_dependencies.insert(
            format!("{}:{}", dep.name.namespace, dep.name.name),
            WitDependency {
                path: format!(
                    "wit/{}",
                    dirs.iter().next().unwrap().to_str().unwrap().to_string()
                ),
            },
        );
    }

    let metadata = MetadataRoot {
        component: Some(ComponentMetadata {
            package: package.clone(),
            target: ComponentTarget {
                world: stub_world_name.clone(),
                path: "wit".to_string(),
                dependencies: wit_dependencies,
            },
        }),
    };

    let mut package = cargo_toml::Package::new(name, version);
    package.edition = Inheritable::Set(Edition::E2021);
    package.metadata = Some(metadata);
    manifest.package = Some(package);

    let lib = cargo_toml::Product {
        path: Some("src/lib.rs".to_string()),
        crate_type: vec!["cdylib".to_string()],
        ..Default::default()
    };
    manifest.lib = Some(lib);

    manifest.profile = Profiles {
        release: Some(Profile {
            lto: Some(LtoSetting::Fat),
            opt_level: Some(Value::String("s".to_string())),
            debug: None,
            split_debuginfo: None,
            rpath: None,
            debug_assertions: None,
            codegen_units: None,
            panic: None,
            incremental: None,
            overflow_checks: None,
            strip: Some(StripSetting::Symbols),
            package: BTreeMap::new(),
            build_override: None,
            inherits: None,
        }),
        ..Default::default()
    };

    let dep_wit_bindgen = Dependency::Detailed(Box::new(DependencyDetail {
        version: Some("0.17.0".to_string()),
        default_features: false,
        features: vec!["realloc".to_string()],
        ..Default::default()
    }));

    // TODO: configurable
    let dep_golem_wasm_rpc = Dependency::Detailed(Box::new(DependencyDetail {
        // version: Some("0.17.0".to_string()),
        path: Some("../../wasm-rpc".to_string()),
        default_features: false,
        features: vec!["stub".to_string()],
        ..Default::default()
    }));

    let mut deps = DepsSet::new();
    deps.insert("wit-bindgen".to_string(), dep_wit_bindgen);
    deps.insert("golem-wasm-rpc".to_string(), dep_golem_wasm_rpc);
    manifest.dependencies = deps;

    let cargo_toml = toml::to_string(&manifest)?;
    fs::write(target, cargo_toml)?;
    Ok(())
}

struct InterfaceStub {
    pub name: String,
    pub functions: Vec<FunctionStub>,
    pub imports: Vec<InterfaceStubImport>,
    pub global: bool,
}

#[derive(Hash, PartialEq, Eq)]
struct InterfaceStubImport {
    pub name: String,
    pub path: String,
}

struct FunctionStub {
    pub name: String,
    pub params: Vec<FunctionParamStub>,
    pub results: FunctionResultStub,
}

struct FunctionParamStub {
    pub name: String,
    pub typ: Type,
}

enum FunctionResultStub {
    Single(Type),
    Multi(Vec<FunctionParamStub>),
}

impl FunctionResultStub {
    pub fn is_empty(&self) -> bool {
        match self {
            FunctionResultStub::Single(_) => false,
            FunctionResultStub::Multi(params) => params.is_empty(),
        }
    }
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
                let name = typ.name.clone().ok_or(anyhow!("type has no name"))?;
                Ok(name)
            }
        }
    }
}

fn collect_stub_imports<'a>(
    types: impl Iterator<Item = (&'a String, &'a TypeId)>,
    resolve: &Resolve,
) -> anyhow::Result<Vec<InterfaceStubImport>> {
    let mut imports = Vec::new();

    for (name, typ) in types {
        println!("type {:?} -> {:?}", name, typ);
        let typ = resolve.types.get(*typ).unwrap();
        println!("  {:?}", typ);
        match typ.owner {
            TypeOwner::World(world_id) => {
                let world = resolve.worlds.get(world_id).unwrap();
                println!("  from world {:?}", world.name);
            }
            TypeOwner::Interface(interface_id) => {
                let interface = resolve.interfaces.get(interface_id).unwrap();
                let package = interface.package.and_then(|id| resolve.packages.get(id));
                let interface_name = interface.name.clone().unwrap_or("unknown".to_string());
                let interface_path = package
                    .map(|p| p.name.interface_id(&interface_name))
                    .unwrap_or(interface_name);
                println!("  from interface {}", interface_path);
                imports.push(InterfaceStubImport {
                    name: name.clone(),
                    path: interface_path,
                });
            }
            TypeOwner::None => {
                println!("  no owner");
            }
        }
    }

    Ok(imports)
}

fn collect_stub_interfaces(resolve: &Resolve, world: &World) -> anyhow::Result<Vec<InterfaceStub>> {
    let top_level_types = world
        .exports
        .iter()
        .filter_map(|(name, item)| match item {
            WorldItem::Type(t) => Some((name.clone().unwrap_name(), t)),
            _ => None,
        })
        .collect::<Vec<_>>();

    let top_level_functions = world
        .exports
        .iter()
        .filter_map(|(_, item)| match item {
            WorldItem::Function(f) => Some(f),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut interfaces = Vec::new();
    for (name, item) in &world.exports {
        match item {
            WorldItem::Interface(id) => {
                let interface = resolve
                    .interfaces
                    .get(*id)
                    .ok_or(anyhow!("exported interface not found"))?;
                let name = interface.name.clone().unwrap_or(String::from(name.clone()));
                let functions = collect_stub_functions(interface.functions.values())?;
                let imports = collect_stub_imports(interface.types.iter(), resolve)?;
                interfaces.push(InterfaceStub {
                    name,
                    functions,
                    imports,
                    global: false,
                });
            }
            _ => {}
        }
    }

    if !top_level_functions.is_empty() {
        interfaces.push(InterfaceStub {
            name: String::from(world.name.clone()),
            functions: collect_stub_functions(top_level_functions.into_iter())?,
            imports: collect_stub_imports(top_level_types.iter().map(|(k, v)| (k, *v)), resolve)?,
            global: true,
        });
    }

    Ok(interfaces)
}

fn collect_stub_functions<'a>(
    functions: impl Iterator<Item = &'a Function>,
) -> anyhow::Result<Vec<FunctionStub>> {
    Ok(functions
        .filter(|f| f.kind == FunctionKind::Freestanding)
        .map(|f| {
            let mut params = Vec::new();
            for (name, typ) in &f.params {
                params.push(FunctionParamStub {
                    name: name.clone(),
                    typ: typ.clone(),
                });
            }

            let results = match &f.results {
                Results::Named(params) => {
                    let mut param_stubs = Vec::new();
                    for (name, typ) in params {
                        param_stubs.push(FunctionParamStub {
                            name: name.clone(),
                            typ: typ.clone(),
                        });
                    }
                    FunctionResultStub::Multi(param_stubs)
                }
                Results::Anon(single) => FunctionResultStub::Single(single.clone()),
            };

            FunctionStub {
                name: f.name.clone(),
                params,
                results,
            }
        })
        .collect())
}

fn generate_stub_wit(
    resolve: &Resolve,
    package_name: PackageName,
    world_id: Id<World>,
    target_world_name: String,
    target: &Path,
) -> anyhow::Result<()> {
    let world = resolve.worlds.get(world_id).unwrap();

    let mut out = String::new();

    writeln!(out, "package {};", package_name)?;
    writeln!(out, "")?;
    writeln!(out, "interface stub-{} {{", world.name)?;

    let interfaces = collect_stub_interfaces(resolve, world)?;
    let all_imports = interfaces
        .iter()
        .flat_map(|i| i.imports.iter())
        .collect::<IndexSet<_>>();

    writeln!(out, "  use golem:rpc/types@0.1.0.{{uri}};")?;
    for import in all_imports {
        writeln!(out, "  use {}.{{{}}};", import.path, import.name)?;
    }
    writeln!(out, "")?;

    for interface in interfaces {
        writeln!(out, "  resource {} {{", &interface.name)?;
        writeln!(out, "    constructor(location: uri);")?;
        for function in interface.functions {
            write!(out, "    {}: func(", function.name)?;
            for (idx, param) in function.params.iter().enumerate() {
                write!(
                    out,
                    "{}: {}",
                    param.name,
                    param.typ.wit_type_string(resolve)?
                )?;
                if idx < function.params.len() - 1 {
                    write!(out, ", ")?;
                }
            }
            write!(out, ")")?;
            if !function.results.is_empty() {
                write!(out, " -> ")?;
                match function.results {
                    FunctionResultStub::Single(typ) => {
                        write!(out, "{}", typ.wit_type_string(resolve)?)?;
                    }
                    FunctionResultStub::Multi(params) => {
                        write!(out, "(")?;
                        for (idx, param) in params.iter().enumerate() {
                            write!(
                                out,
                                "{}: {}",
                                param.name,
                                param.typ.wit_type_string(resolve)?
                            )?;
                            if idx < params.len() - 1 {
                                write!(out, ", ")?;
                            }
                        }
                        write!(out, ")")?;
                    }
                }
            }
            writeln!(out, ";")?;
        }
        writeln!(out, "  }}")?;
        writeln!(out, "")?;
    }

    writeln!(out, "}}")?;
    writeln!(out, "")?;

    writeln!(out, "world {} {{", target_world_name)?;
    writeln!(out, "  export stub-{};", world.name)?;
    writeln!(out, "}}")?;

    fs::write(target, out)?;
    Ok(())
}

fn dump(resolve: &Resolve) {
    for (id, world) in &resolve.worlds {
        println!("World {id:?}");
        for (key, item) in &world.exports {
            println!("  {key:?} -> {item:?}");

            match item {
                WorldItem::Interface(id) => {
                    if let Some(interface) = resolve.interfaces.get(*id) {
                        println!("    interface {:?}", &interface.name);

                        for (_, f) in &interface.functions {
                            println!("      function {:?}", f.name);
                            for (name, typ) in &f.params {
                                println!("        param {:?} -> {:?}", name, typ);
                            }
                            for typ in f.results.iter_types() {
                                println!("        result {:?}", typ);
                            }
                        }
                    }
                }
                WorldItem::Function(f) => {
                    println!("    function {:?}", f.name);
                    for (name, typ) in &f.params {
                        println!("      param {:?} -> {:?}", name, typ);
                    }
                    for typ in f.results.iter_types() {
                        println!("      result {:?}", typ);
                    }
                }
                WorldItem::Type(id) => {
                    let ty = resolve.types.get(*id);
                    println!("    type {:?}", ty.map(|t| &t.name));
                }
            }
        }
    }
}

fn generate_stub_source(
    target: &Path,
    resolve: &Resolve,
    world_id: &WorldId,
    root_package: PackageName,
) -> anyhow::Result<()> {
    let world = resolve.worlds.get(*world_id).unwrap();
    let interfaces = collect_stub_interfaces(resolve, world)?;

    let root_ns = Ident::new(&root_package.namespace, Span::call_site());
    let root_name = Ident::new(&root_package.name, Span::call_site());

    let mut struct_defs = Vec::new();
    for interface in &interfaces {
        let interface_ident = to_rust_ident(&interface.name).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());
        struct_defs.push(quote! {
           pub struct #interface_name {
                rpc: WasmRpc,
            }
        });
    }

    let mut interface_impls = Vec::new();
    for interface in &interfaces {
        let interface_ident = to_rust_ident(&interface.name).to_upper_camel_case();
        let interface_name = Ident::new(&interface_ident, Span::call_site());
        let guest_interface_name =
            Ident::new(&format!("Guest{}", interface_ident), Span::call_site());

        let mut fn_impls = Vec::new();
        for function in &interface.functions {
            fn_impls.push(generate_function_stub_source(
                &function,
                &root_ns,
                &root_name,
                if interface.global {
                    None
                } else {
                    Some(&interface.name)
                },
                resolve,
            )?);
        }

        interface_impls.push(quote! {
            impl crate::bindings::exports::#root_ns::#root_name::stub_api::#guest_interface_name for #interface_name {
                fn new(location: crate::bindings::golem::rpc::types::Uri) -> Self {
                    let location = Uri { value: location.value };
                    Self {
                        rpc: WasmRpc::new(&location)
                    }
                }

                #(#fn_impls)*
            }
        });
    }

    let lib = quote! {
        use golem_wasm_rpc::*;

        #[allow(dead_code)]
        mod bindings;

        #(#struct_defs)*

        #(#interface_impls)*
    };
    println!("{}", lib);
    let syntax_tree = syn::parse2(lib)?;
    let src = prettyplease::unparse(&syntax_tree);

    fs::write(target, src)?;
    Ok(())
}

fn generate_function_stub_source(
    function: &FunctionStub,
    root_ns: &Ident,
    root_name: &Ident,
    interface_name: Option<&String>,
    resolve: &Resolve,
) -> anyhow::Result<TokenStream> {
    let function_name = Ident::new(&function.name, Span::call_site());
    let mut params = Vec::new();
    let mut input_values = Vec::new();

    for param in &function.params {
        let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
        let param_typ = type_to_rust_ident(&param.typ, root_ns, root_name, resolve)?;
        params.push(quote! {
            #param_name: #param_typ
        });

        input_values.push(wit_value_builder(&param.typ, &param_name, resolve)?);
    }

    let result_type = match &function.results {
        FunctionResultStub::Single(typ) => {
            let typ = type_to_rust_ident(&typ, root_ns, root_name, resolve)?;
            quote! {
                #typ
            }
        }
        FunctionResultStub::Multi(params) => {
            let mut results = Vec::new();
            for param in params {
                let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
                let param_typ = type_to_rust_ident(&param.typ, root_ns, root_name, resolve)?;
                results.push(quote! {
                    #param_name: #param_typ
                });
            }
            if results.is_empty() {
                quote! {
                    ()
                }
            } else {
                quote! {
                    (#(#results),*)
                }
            }
        }
    };

    let remote_function_name = match interface_name {
        Some(remote_interface) => format!(
            "{}:{}/{}/{}",
            root_ns, root_name, remote_interface, function.name
        ),
        None => format!("{}:{}/{}", root_ns, root_name, function.name),
    };

    Ok(quote! {
        fn #function_name(&self, #(#params),*) -> #result_type {
            let result = self.rpc.invoke_and_await(
                #remote_function_name,
                &[
                    #(#input_values),*
                ],
            ).expect(&format!("Failed to invoke remote {}", #remote_function_name));
            todo!() // TODO
        }
    })
}

fn type_to_rust_ident(
    typ: &Type,
    root_ns: &Ident,
    root_name: &Ident,
    resolve: &Resolve,
) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! { bool }),
        Type::U8 => Ok(quote! { u8 }),
        Type::U16 => Ok(quote! { u16 }),
        Type::U32 => Ok(quote! { u32 }),
        Type::U64 => Ok(quote! { u64 }),
        Type::S8 => Ok(quote! { i8 }),
        Type::S16 => Ok(quote! { i16 }),
        Type::S32 => Ok(quote! { i32 }),
        Type::S64 => Ok(quote! { i64 }),
        Type::Float32 => Ok(quote! { f32 }),
        Type::Float64 => Ok(quote! { f64 }),
        Type::Char => Ok(quote! { char }),
        Type::String => Ok(quote! { String }),
        Type::Id(type_id) => {
            let typ = Ident::new(
                &to_rust_ident(
                    &resolve
                        .types
                        .get(*type_id)
                        .ok_or(anyhow!("type not found"))?
                        .name
                        .clone()
                        .ok_or(anyhow!("type has no name"))?,
                )
                .to_upper_camel_case(),
                Span::call_site(),
            );
            Ok(quote! { crate::bindings::exports::#root_ns::#root_name::stub_api::#typ })
        }
    }
}

fn wit_value_builder(typ: &Type, name: &Ident, _resolve: &Resolve) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! {
            WitValue::builder().bool(#name)
        }),
        Type::U8 => Ok(quote! {
            WitValue::builder().u8(#name)
        }),
        Type::U16 => Ok(quote! {
            WitValue::builder().u16(#name)
        }),
        Type::U32 => Ok(quote! {
            WitValue::builder().u32(#name)
        }),
        Type::U64 => Ok(quote! {
            WitValue::builder().u64(#name)
        }),
        Type::S8 => Ok(quote! {
            WitValue::builder().s8(#name)
        }),
        Type::S16 => Ok(quote! {
            WitValue::builder().s16(#name)
        }),
        Type::S32 => Ok(quote! {
            WitValue::builder().s32(#name)
        }),
        Type::S64 => Ok(quote! {
            WitValue::builder().s64(#name)
        }),
        Type::Float32 => Ok(quote! {
            WitValue::builder().f32(#name)
        }),
        Type::Float64 => Ok(quote! {
            WitValue::builder().f64(#name)
        }),
        Type::Char => Ok(quote! {
            WitValue::builder().char(#name)
        }),
        Type::String => Ok(quote! {
            WitValue::builder().string(&#name)
        }),
        Type::Id(_) => Ok(quote!(todo!())), // TODO
    }
}
