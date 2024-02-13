use anyhow::{anyhow, bail};
use cargo_toml::{
    Dependency, DependencyDetail, DepsSet, Edition, Inheritable, LtoSetting, Manifest, Profile,
    Profiles, StripSetting,
};
use heck::{ToShoutySnakeCase, ToUpperCamelCase};
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
        visit(pkg, &deps, &mut order, &mut visiting)?;
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
        if unresolved.name == root_package {
            println!("copying root {:?}", unresolved.name);
            let dep_dir = dest_wit_root
                .join(Path::new("deps"))
                .join(Path::new(&format!(
                    "{}_{}",
                    root_package.namespace, root_package.name
                )));
            fs::create_dir_all(&dep_dir).unwrap();
            for source in unresolved.source_files() {
                let dest = dep_dir.join(source.file_name().unwrap());
                println!("Copying {source:?} to {dest:?}");
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                fs::copy(source, &dest).unwrap();
            }
        } else {
            println!("copying {:?}", unresolved.name);
            for source in unresolved.source_files() {
                let relative = source.strip_prefix(root_path).unwrap();
                let dest = dest_wit_root.join(relative);
                println!("Copying {source:?} to {dest:?}");
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                fs::copy(source, &dest).unwrap();
            }
        }
    }
    let wasm_rpc_wit = include_str!("../../wasm-rpc/wit/wasm-rpc.wit");
    let wasm_rpc_root = dest_wit_root.join(Path::new("deps/wasm-rpc"));
    fs::create_dir_all(&wasm_rpc_root).unwrap();
    fs::write(wasm_rpc_root.join(Path::new("wasm-rpc.wit")), wasm_rpc_wit).unwrap();

    println!("----");

    let (final_root, final_deps) = get_unresolved_packages(&dest_wit_root).unwrap();

    let mut final_resolve = Resolve::new();
    for unresolved in final_deps.iter().cloned() {
        final_resolve.push(unresolved).unwrap();
    }
    final_resolve.push(final_root.clone()).unwrap();
    dump(&final_resolve);

    println!("generating cargo.toml");
    generate_cargo_toml(
        root_path,
        &dest_root.join("Cargo.toml"),
        selected_world,
        stub_crate_version,
        &root_package,
        stub_world_name,
        &deps,
    )
    .unwrap();

    let dest_src_root = dest_root.join(Path::new("src"));
    fs::create_dir_all(dest_src_root).unwrap();
    generate_stub_source(
        &dest_root.join("src/lib.rs"),
        &resolve,
        &world,
        root_package,
    )
    .unwrap();
}

#[derive(Serialize, Default)]
struct MetadataRoot {
    component: Option<ComponentMetadata>,
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
    root_package: &PackageName,
    stub_world_name: String,
    deps: &[UnresolvedPackage],
) -> anyhow::Result<()> {
    let mut manifest = Manifest::default();

    let mut wit_dependencies = HashMap::new();

    wit_dependencies.insert(
        root_package.to_string(),
        WitDependency {
            path: format!("wit/deps/{}_{}", root_package.namespace, root_package.name),
        },
    );
    wit_dependencies.insert(
        "golem:rpc".to_string(),
        WitDependency {
            path: "wit/deps/wasm-rpc".to_string(),
        },
    );
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
            format!("{}:{}", dep.name.namespace, dep.name.name),
            WitDependency {
                path: format!("wit/{}", dirs.iter().next().unwrap().to_str().unwrap()),
            },
        );
    }

    let metadata = MetadataRoot {
        component: Some(ComponentMetadata {
            package: format!("{}:{}", root_package.namespace, root_package.name),
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
            name: world.name.clone(),
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
                    typ: *typ,
                });
            }

            let results = match &f.results {
                Results::Named(params) => {
                    let mut param_stubs = Vec::new();
                    for (name, typ) in params {
                        param_stubs.push(FunctionParamStub {
                            name: name.clone(),
                            typ: *typ,
                        });
                    }
                    FunctionResultStub::Multi(param_stubs)
                }
                Results::Anon(single) => FunctionResultStub::Single(*single),
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

    writeln!(out, "package {}-stub;", package_name)?;
    writeln!(out)?;
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
    writeln!(out)?;

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
        writeln!(out)?;
    }

    writeln!(out, "}}")?;
    writeln!(out)?;

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
    let root_name = Ident::new(&format!("{}_stub", root_package.name), Span::call_site());

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
                function,
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
    let function_name = Ident::new(&to_rust_ident(&function.name), Span::call_site());
    let mut params = Vec::new();
    let mut input_values = Vec::new();
    let mut output_values = Vec::new();

    for param in &function.params {
        let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
        let param_typ = type_to_rust_ident(&param.typ, resolve)?;
        params.push(quote! {
            #param_name: #param_typ
        });
        let param_name_access = quote! { #param_name };

        input_values.push(wit_value_builder(
            &param.typ,
            &param_name_access,
            resolve,
            quote! { WitValue::builder() },
        )?);
    }

    let result_type = match &function.results {
        FunctionResultStub::Single(typ) => {
            let typ = type_to_rust_ident(typ, resolve)?;
            quote! {
                #typ
            }
        }
        FunctionResultStub::Multi(params) => {
            let mut results = Vec::new();
            for param in params {
                let param_name = Ident::new(&to_rust_ident(&param.name), Span::call_site());
                let param_typ = type_to_rust_ident(&param.typ, resolve)?;
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

    match &function.results {
        FunctionResultStub::Single(typ) => {
            output_values.push(extract_from_wit_value(typ, resolve, quote! { result })?);
        }
        FunctionResultStub::Multi(params) => {
            for (n, param) in params.iter().enumerate() {
                output_values.push(extract_from_wit_value(
                    &param.typ,
                    resolve,
                    quote! { result.tuple_element(#n).expect("tuple not found") },
                )?);
            }
        }
    }

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
            (#(#output_values),*)
        }
    })
}

fn type_to_rust_ident(typ: &Type, resolve: &Resolve) -> anyhow::Result<TokenStream> {
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
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;

            match &typedef.kind {
                TypeDefKind::Option(inner) => {
                    let inner = type_to_rust_ident(inner, resolve)?;
                    Ok(quote! { Option<#inner> })
                }
                TypeDefKind::List(inner) => {
                    let inner = type_to_rust_ident(inner, resolve)?;
                    Ok(quote! { Vec<#inner> })
                }
                TypeDefKind::Tuple(tuple) => {
                    let types = tuple
                        .types
                        .iter()
                        .map(|t| type_to_rust_ident(t, resolve))
                        .collect::<anyhow::Result<Vec<_>>>()?;
                    Ok(quote! { (#(#types),*) })
                }
                TypeDefKind::Result(result) => {
                    let ok = match &result.ok {
                        Some(ok) => type_to_rust_ident(ok, resolve)?,
                        None => quote! { () },
                    };
                    let err = match &result.err {
                        Some(err) => type_to_rust_ident(err, resolve)?,
                        None => quote! { () },
                    };
                    Ok(quote! { Result<#ok, #err> })
                }
                _ => {
                    let typ = Ident::new(
                        &to_rust_ident(typedef.name.as_ref().ok_or(anyhow!("type has no name"))?)
                            .to_upper_camel_case(),
                        Span::call_site(),
                    );
                    let mut path = Vec::new();
                    path.push(quote! { crate });
                    path.push(quote! { bindings });
                    match &typedef.owner {
                        TypeOwner::World(world_id) => {
                            let world = resolve
                                .worlds
                                .get(*world_id)
                                .ok_or(anyhow!("type's owner world not found"))?;
                            let package_id =
                                world.package.ok_or(anyhow!("world has no package"))?;
                            let package = resolve
                                .packages
                                .get(package_id)
                                .ok_or(anyhow!("package not found"))?;
                            let ns_ident = Ident::new(
                                &to_rust_ident(&package.name.namespace),
                                Span::call_site(),
                            );
                            let name_ident =
                                Ident::new(&to_rust_ident(&package.name.name), Span::call_site());
                            path.push(quote! { #ns_ident });
                            path.push(quote! { #name_ident });
                        }
                        TypeOwner::Interface(interface_id) => {
                            let interface = resolve
                                .interfaces
                                .get(*interface_id)
                                .ok_or(anyhow!("type's owner interface not found"))?;

                            let package_id = interface
                                .package
                                .ok_or(anyhow!("interface has no package"))?;
                            let package = resolve
                                .packages
                                .get(package_id)
                                .ok_or(anyhow!("package not found"))?;
                            let interface_name = interface
                                .name
                                .as_ref()
                                .ok_or(anyhow!("interface has no name"))?;
                            let ns_ident = Ident::new(
                                &to_rust_ident(&package.name.namespace),
                                Span::call_site(),
                            );
                            let name_ident =
                                Ident::new(&to_rust_ident(&package.name.name), Span::call_site());
                            let interface_ident =
                                Ident::new(&to_rust_ident(interface_name), Span::call_site());
                            path.push(quote! { #ns_ident });
                            path.push(quote! { #name_ident });
                            path.push(quote! { #interface_ident });
                        }
                        TypeOwner::None => {}
                    }
                    Ok(quote! { #(#path)::*::#typ })
                }
            }
        }
    }
}

fn wit_value_builder(
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! {
            #builder_expr.bool(#name)
        }),
        Type::U8 => Ok(quote! {
            #builder_expr.u8(#name)
        }),
        Type::U16 => Ok(quote! {
            #builder_expr.u16(#name)
        }),
        Type::U32 => Ok(quote! {
            #builder_expr.u32(#name)
        }),
        Type::U64 => Ok(quote! {
            #builder_expr.u64(#name)
        }),
        Type::S8 => Ok(quote! {
            #builder_expr.s8(#name)
        }),
        Type::S16 => Ok(quote! {
            #builder_expr.s16(#name)
        }),
        Type::S32 => Ok(quote! {
            #builder_expr.s32(#name)
        }),
        Type::S64 => Ok(quote! {
            #builder_expr.s64(#name)
        }),
        Type::Float32 => Ok(quote! {
            #builder_expr.f32(#name)
        }),
        Type::Float64 => Ok(quote! {
            #builder_expr.f64(#name)
        }),
        Type::Char => Ok(quote! {
            #builder_expr.char(#name)
        }),
        Type::String => Ok(quote! {
            #builder_expr.string(&#name)
        }),
        Type::Id(type_id) => {
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;
            match &typedef.kind {
                TypeDefKind::Record(record) => {
                    wit_record_value_builder(record, name, resolve, builder_expr)
                }
                TypeDefKind::Resource => Ok(quote!(todo!("resource support"))),
                TypeDefKind::Handle(_) => Ok(quote!(todo!("resource support"))),
                TypeDefKind::Flags(flags) => {
                    wit_flags_value_builder(flags, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Tuple(tuple) => {
                    wit_tuple_value_builder(tuple, name, resolve, builder_expr)
                }
                TypeDefKind::Variant(variant) => {
                    wit_variant_value_builder(variant, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    wit_enum_value_builder(enum_def, typ, name, resolve, builder_expr)
                }
                TypeDefKind::Option(inner) => {
                    wit_option_value_builder(inner, name, resolve, builder_expr)
                }
                TypeDefKind::Result(result) => {
                    wit_result_value_builder(result, name, resolve, builder_expr)
                }
                TypeDefKind::List(elem) => {
                    wit_list_value_builder(elem, name, resolve, builder_expr)
                }
                TypeDefKind::Future(_) => {
                    Ok(quote!(todo!("future"))) // TODO
                }
                TypeDefKind::Stream(_) => {
                    Ok(quote!(todo!("stream"))) // TODO
                }
                TypeDefKind::Type(typ) => wit_value_builder(typ, name, resolve, builder_expr),
                TypeDefKind::Unknown => {
                    Ok(quote!(todo!("unknown"))) // TODO
                }
            }
        }
    }
}

fn wit_record_value_builder(
    record: &Record,
    name: &TokenStream,
    resolve: &Resolve,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.record() };

    for field in &record.fields {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_access = quote! { #name.#field_name };
        builder_expr = wit_value_builder(
            &field.ty,
            &field_access,
            resolve,
            quote! { #builder_expr.item() },
        )?;
    }

    Ok(quote! { #builder_expr.finish() })
}

fn wit_flags_value_builder(
    flags: &Flags,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(typ, resolve)?;

    let mut flags_vec_values = Vec::new();
    for flag in &flags.flags {
        let flag_id = Ident::new(&flag.name.to_shouty_snake_case(), Span::call_site());
        flags_vec_values.push(quote! {
            (#name & #flags_type::#flag_id) == #flags_type::#flag_id
        })
    }

    Ok(quote! { #builder_expr.flags(vec![#(#flags_vec_values),*]) })
}

fn wit_tuple_value_builder(
    tuple: &Tuple,
    name: &TokenStream,
    resolve: &Resolve,
    mut builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    builder_expr = quote! { #builder_expr.tuple() };

    for (n, typ) in tuple.types.iter().enumerate() {
        let field_name = syn::Index::from(n);
        let field_access = quote! { #name.#field_name };
        builder_expr =
            wit_value_builder(typ, &field_access, resolve, quote! { #builder_expr.item() })?;
    }

    Ok(quote! { #builder_expr.finish() })
}

fn wit_variant_value_builder(
    variant: &Variant,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(typ, resolve)?;

    let mut case_idx_patterns = Vec::new();
    let mut is_unit_patterns = Vec::new();
    let mut builder_patterns = Vec::new();
    for (n, case) in variant.cases.iter().enumerate() {
        let case_name = Ident::new(
            &to_rust_ident(&case.name).to_upper_camel_case(),
            Span::call_site(),
        );

        let case_idx: u32 = n as u32;

        match &case.ty {
            None => {
                case_idx_patterns.push(quote! {
                    #variant_type::#case_name => #case_idx
                });
                is_unit_patterns.push(quote! {
                    #variant_type::#case_name => true
                });
                builder_patterns.push(quote! {
                    #variant_type::#case_name => {
                        unreachable!()
                    }
                });
            }
            Some(inner_ty) => {
                let inner_builder_expr = wit_value_builder(
                    inner_ty,
                    &quote! { inner },
                    resolve,
                    quote! { case_builder },
                )?;

                case_idx_patterns.push(quote! {
                    #variant_type::#case_name(_) => #case_idx
                });
                is_unit_patterns.push(quote! {
                    #variant_type::#case_name(_) => false
                });
                builder_patterns.push(quote! {
                    #variant_type::#case_name(inner) => {
                        #inner_builder_expr
                    }
                });
            }
        }
    }

    Ok(quote! {
        #builder_expr.variant_fn(
            match &#name {
                #(#case_idx_patterns),*,
            },
            match &#name {
                #(#is_unit_patterns),*,
            },
            |case_builder| match &#name {
                #(#builder_patterns),*,
            }
        )
    })
}

fn wit_enum_value_builder(
    enum_def: &Enum,
    typ: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(typ, resolve)?;

    let mut cases = Vec::new();
    for (n, case) in enum_def.cases.iter().enumerate() {
        let case_name = Ident::new(&case.name.to_shouty_snake_case(), Span::call_site());
        let case_idx = n as u32;
        cases.push(quote! {
            #enum_type::#case_name => #case_idx
        });
    }

    Ok(quote! {
        #builder_expr.enum_value(match #name {
            #(#cases),*
        })
    })
}

fn wit_option_value_builder(
    inner: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr = wit_value_builder(
        inner,
        &quote! { #name.as_ref().unwrap() },
        resolve,
        quote! { some_builder },
    )?;

    Ok(quote! {
        #builder_expr.option_fn(#name.is_some(), |some_builder| {
            #inner_builder_expr
        })
    })
}

fn wit_result_value_builder(
    result: &Result_,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => {
            wit_value_builder(ok, &quote! { ok_value }, resolve, quote! { result_builder })?
        }
        None => quote! { unreachable!() },
    };
    let err_expr = match &result.err {
        Some(err) => wit_value_builder(
            err,
            &quote! { err_value },
            resolve,
            quote! { result_builder },
        )?,
        None => quote! { unreachable!() },
    };

    let has_ok = result.ok.is_some();
    let has_err = result.err.is_some();
    Ok(quote! {
        #builder_expr.result_fn(#name.is_ok(), #has_ok, #has_err, |result_builder| {
            match &#name {
                Ok(ok_value) => {
                    #ok_expr
                }
                Err(err_value) => {
                    #err_expr
                }
            }
        })
    })
}

fn wit_list_value_builder(
    inner: &Type,
    name: &TokenStream,
    resolve: &Resolve,
    builder_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_builder_expr =
        wit_value_builder(inner, &quote! { item }, resolve, quote! { item_builder })?;

    Ok(quote! {
        #builder_expr.list_fn(&#name, |item, item_builder| {
            #inner_builder_expr
        })
    })
}

fn extract_from_wit_value(
    typ: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    match typ {
        Type::Bool => Ok(quote! {
            #base_expr.bool().expect("bool not found")
        }),
        Type::U8 => Ok(quote! {
            #base_expr.u8().expect("u8 not found")
        }),
        Type::U16 => Ok(quote! {
            #base_expr.u16().expect("u16 not found")
        }),
        Type::U32 => Ok(quote! {
            #base_expr.u32().expect("u32 not found")
        }),
        Type::U64 => Ok(quote! {
            #base_expr.u64().expect("u64 not found")
        }),
        Type::S8 => Ok(quote! {
            #base_expr.s8().expect("i8 not found")
        }),
        Type::S16 => Ok(quote! {
            #base_expr.s16().expect("i16 not found")
        }),
        Type::S32 => Ok(quote! {
            #base_expr.s32().expect("i32 not found")
        }),
        Type::S64 => Ok(quote! {
            #base_expr.s64().expect("i64 not found")
        }),
        Type::Float32 => Ok(quote! {
            #base_expr.f32().expect("f32 not found")
        }),
        Type::Float64 => Ok(quote! {
            #base_expr.f64().expect("f64 not found")
        }),
        Type::Char => Ok(quote! {
            #base_expr.char().expect("char not found")
        }),
        Type::String => Ok(quote! {
            #base_expr.string().expect("string not found").to_string()
        }),
        Type::Id(type_id) => {
            let typedef = resolve
                .types
                .get(*type_id)
                .ok_or(anyhow!("type not found"))?;
            match &typedef.kind {
                TypeDefKind::Record(record) => {
                    extract_from_record_value(record, typ, resolve, base_expr)
                }
                TypeDefKind::Resource => Ok(quote!(todo!("resource support"))),
                TypeDefKind::Handle(_) => Ok(quote!(todo!("resource support"))),
                TypeDefKind::Flags(flags) => {
                    extract_from_flags_value(flags, typ, resolve, base_expr)
                }
                TypeDefKind::Tuple(tuple) => extract_from_tuple_value(tuple, resolve, base_expr),
                TypeDefKind::Variant(variant) => {
                    extract_from_variant_value(variant, typ, resolve, base_expr)
                }
                TypeDefKind::Enum(enum_def) => {
                    extract_from_enum_value(enum_def, typ, resolve, base_expr)
                }
                TypeDefKind::Option(inner) => extract_from_option_value(inner, resolve, base_expr),
                TypeDefKind::Result(result) => {
                    extract_from_result_value(result, resolve, base_expr)
                }
                TypeDefKind::List(elem) => extract_from_list_value(elem, resolve, base_expr),
                TypeDefKind::Future(_) => {
                    Ok(quote!(todo!("future"))) // TODO
                }
                TypeDefKind::Stream(_) => {
                    Ok(quote!(todo!("stream"))) // TODO
                }
                TypeDefKind::Type(typ) => extract_from_wit_value(typ, resolve, base_expr),
                TypeDefKind::Unknown => {
                    Ok(quote!(todo!("unknown"))) // TODO
                }
            }
        }
    }
}

fn extract_from_record_value(
    record: &Record,
    record_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut field_extractors = Vec::new();
    for (field_idx, field) in record.fields.iter().enumerate() {
        let field_name = Ident::new(&to_rust_ident(&field.name), Span::call_site());
        let field_expr = extract_from_wit_value(
            &field.ty,
            resolve,
            quote! { record.field(#field_idx).expect("record field not found") },
        )?;
        field_extractors.push(quote! {
            #field_name: #field_expr
        });
    }

    let record_type = type_to_rust_ident(&record_type, resolve)?;

    Ok(quote! {
        {
            let record = #base_expr;
            #record_type {
                #(#field_extractors),*
            }
        }
    })
}

fn extract_from_flags_value(
    flags: &Flags,
    flags_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let flags_type = type_to_rust_ident(&flags_type, resolve)?;
    let mut flag_exprs = Vec::new();
    for (flag_idx, flag) in flags.flags.iter().enumerate() {
        let flag_name = Ident::new(&flag.name.to_shouty_snake_case(), Span::call_site());
        flag_exprs.push(quote! {
            if flag_vec[#flag_idx] {
                flags |= #flags_type::#flag_name;
            }
        });
    }

    Ok(quote! {
        {
            let flag_vec = #base_expr.flags().expect("flags not found");
            let mut flags = #flags_type::empty();
            #(#flag_exprs);*
            flags
        }
    })
}

fn extract_from_tuple_value(
    tuple: &Tuple,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let mut elem_extractors = Vec::new();
    for (field_idx, typ) in tuple.types.iter().enumerate() {
        let elem_expr = extract_from_wit_value(
            typ,
            resolve,
            quote! { tuple.tuple_element(#field_idx).expect("tuple element not found") },
        )?;
        elem_extractors.push(elem_expr);
    }

    Ok(quote! {
        {
            let tuple = #base_expr;
            (#(#elem_extractors),*)
        }
    })
}

fn extract_from_variant_value(
    variant: &Variant,
    variant_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let variant_type = type_to_rust_ident(&variant_type, resolve)?;

    let mut case_extractors = Vec::new();
    for (n, case) in variant.cases.iter().enumerate() {
        let case_name = Ident::new(
            &to_rust_ident(&case.name).to_upper_camel_case(),
            Span::call_site(),
        );
        let case_idx = n as u32;

        match &case.ty {
            Some(ty) => {
                let case_expr = extract_from_wit_value(
                    ty,
                    resolve,
                    quote! { inner.expect("variant case not found") },
                )?;
                case_extractors.push(quote! {
                    #case_idx => #variant_type::#case_name(#case_expr)
                });
            }
            None => {
                case_extractors.push(quote! {
                    #case_idx => #variant_type::#case_name
                });
            }
        }
    }

    Ok(quote! {
        {
            let (case_idx, inner) = #base_expr.variant().expect("variant not found");
            match case_idx {
                #(#case_extractors),*,
                _ => unreachable!("invalid variant case index")
            }
        }
    })
}

fn extract_from_enum_value(
    enum_def: &Enum,
    enum_type: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let enum_type = type_to_rust_ident(&enum_type, resolve)?;

    let mut case_extractors = Vec::new();
    for (n, case) in enum_def.cases.iter().enumerate() {
        let case_name = Ident::new(&case.name.to_shouty_snake_case(), Span::call_site());
        let case_idx = n as u32;
        case_extractors.push(quote! {
            #case_idx => #enum_type::#case_name
        });
    }

    Ok(quote! {
        {
            let case_idx = #base_expr.enum_value().expect("enum not found");
            match case_idx {
                #(#case_extractors),*,
                _ => unreachable!("invalid enum case index")
            }
        }
    })
}

fn extract_from_option_value(
    inner: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, resolve, quote! { inner })?;

    Ok(quote! {
        #base_expr.option().expect("option not found").map(|inner| #inner_expr)
    })
}

fn extract_from_result_value(
    result: &Result_,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let ok_expr = match &result.ok {
        Some(ok) => extract_from_wit_value(
            ok,
            resolve,
            quote! { ok_value.expect("result ok value not found") },
        )?,
        None => quote! { () },
    };
    let err_expr = match &result.err {
        Some(err) => extract_from_wit_value(
            err,
            resolve,
            quote! { err_value.expect("result err value not found") },
        )?,
        None => quote! { () },
    };

    Ok(quote! {
        {
            let result = #base_expr.result().expect("result not found");
            match result {
                Ok(ok_value) => Ok(#ok_expr),
                Err(err_value) => Err(#err_expr)
            }
        }
    })
}

fn extract_from_list_value(
    inner: &Type,
    resolve: &Resolve,
    base_expr: TokenStream,
) -> anyhow::Result<TokenStream> {
    let inner_expr = extract_from_wit_value(inner, resolve, quote! { item })?;

    Ok(quote! {
        #base_expr.list_elements(|item| #inner_expr).expect("list not found")
    })
}
