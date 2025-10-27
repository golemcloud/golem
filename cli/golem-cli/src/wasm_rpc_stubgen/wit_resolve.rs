use crate::app::build::command::ensure_common_deps_for_tool;
use crate::app::context::ToolsWithEnsuredCommonDeps;
use crate::fs;
use crate::fs::PathExtra;
use crate::log::{log_action, LogColorize, LogIndent};
use crate::model::app::{AppComponentName, Application, BuildProfileName};
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::wasm_rpc_stubgen::naming;
use anyhow::{anyhow, bail, Context, Error};
use golem_common::model::agent::AgentType;
use indexmap::IndexMap;
use indoc::formatdoc;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use wit_parser::decoding::DecodedWasm;
use wit_parser::{
    InterfaceId, Package, PackageId, PackageName, PackageSourceMap, Resolve,
    UnresolvedPackageGroup, WorldItem,
};

pub struct PackageSource {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

pub struct ResolvedWitDir {
    pub path: PathBuf,
    pub resolve: Resolve,
    pub package_id: PackageId,
    pub package_sources: IndexMap<PackageId, PackageSource>,
}

impl ResolvedWitDir {
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        resolve_wit_dir(path)
    }

    pub fn from_wasm(path: &Path, wasm: &DecodedWasm) -> Self {
        Self {
            path: path.to_path_buf(),
            resolve: wasm.resolve().clone(),
            package_id: wasm.package(),
            package_sources: IndexMap::new(),
        }
    }

    pub fn package(&self, package_id: PackageId) -> anyhow::Result<&Package> {
        self.resolve.packages.get(package_id).with_context(|| {
            anyhow!(
                "Failed to get package by id: {:?}, wit dir: {}",
                package_id,
                self.path.log_color_highlight()
            )
        })
    }

    pub fn package_sources(&self, package_id: PackageId) -> Result<&PackageSource, Error> {
        self.package_sources.get(&package_id).with_context(|| {
            anyhow!(
                "Failed to get package sources by id: {:?}, wit dir: {}",
                package_id,
                self.path.log_color_highlight()
            )
        })
    }

    pub fn main_package(&self) -> anyhow::Result<&Package> {
        self.package(self.package_id)
    }

    pub fn used_interfaces(&self) -> anyhow::Result<HashSet<(InterfaceId, PackageName, String)>> {
        let mut result = HashSet::new();
        let main = self.main_package()?;
        for (world_name, world_id) in &main.worlds {
            let world = self
                .resolve
                .worlds
                .get(*world_id)
                .ok_or_else(|| anyhow!("Could not find world {world_name} in resolve"))?;
            for (key, item) in world.imports.iter().chain(world.exports.iter()) {
                if let WorldItem::Interface { id, .. } = item {
                    let interface =
                        self.resolve.interfaces.get(*id).ok_or_else(|| {
                            anyhow!("Could not find interface {key:?} in resolve")
                        })?;
                    if let Some(package_id) = interface.package {
                        let package = self.resolve.packages.get(package_id).ok_or_else(|| {
                            anyhow!("Could not find package {package_id:?} in resolve")
                        })?;
                        if let Some(interface_name) = &interface.name {
                            result.insert((*id, package.name.clone(), interface_name.clone()));
                        }
                    }
                }
            }
        }

        Ok(result)
    }

    pub fn exported_functions(&self) -> anyhow::Result<Vec<ExportedFunction>> {
        let mut result = Vec::new();
        let main = self.main_package()?;
        for (world_name, world_id) in &main.worlds {
            let world = self
                .resolve
                .worlds
                .get(*world_id)
                .ok_or_else(|| anyhow!("Could not find world {world_name} in resolve"))?;
            for (export_name, export) in world.exports.iter() {
                match export {
                    WorldItem::Function(function) => {
                        let world_name = self.resolve.worlds[*world_id].name.clone();
                        result.push(ExportedFunction::InlineFunction {
                            world_name,
                            function_name: function.name.clone(),
                        });
                    }
                    WorldItem::Interface { id, .. } => {
                        let iface = &self.resolve.interfaces[*id];
                        match export_name {
                            // inline interface
                            wit_parser::WorldKey::Name(export_name) => {
                                for (function_name, _) in iface.functions.iter() {
                                    result.push(ExportedFunction::InlineInterface {
                                        export_name: export_name.clone(),
                                        function_name: function_name.clone(),
                                    });
                                }
                            }
                            // external interface
                            wit_parser::WorldKey::Interface(iface_id) => {
                                let interface_name = self.resolve.id_of(*iface_id).unwrap();
                                for (function_name, _) in iface.functions.iter() {
                                    result.push(ExportedFunction::Interface {
                                        interface_name: interface_name.clone(),
                                        function_name: function_name.clone(),
                                    });
                                }
                            }
                        }
                    }
                    WorldItem::Type(_) => {}
                }
            }
        }

        Ok(result)
    }

    pub fn is_agent(&self) -> anyhow::Result<bool> {
        golem_common::model::agent::extraction::is_agent(&self.resolve, &self.package_id, None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExportedFunction {
    Interface {
        interface_name: String,
        function_name: String,
    },
    InlineInterface {
        export_name: String,
        function_name: String,
    },
    InlineFunction {
        world_name: String,
        function_name: String,
    },
}

fn resolve_wit_dir(path: &Path) -> anyhow::Result<ResolvedWitDir> {
    // TODO: Can be removed once we fixed all docs and templates
    std::env::set_var("WIT_REQUIRE_F32_F64", "0");

    let mut resolve = Resolve::new();

    let (package_id, package_source_map) = resolve
        .push_dir(path)
        .with_context(|| anyhow!("Failed to resolve wit dir: {}", path.log_color_highlight()))?;

    let package_sources = collect_package_sources(path, &resolve, package_id, &package_source_map)?;

    Ok(ResolvedWitDir {
        path: path.to_path_buf(),
        resolve,
        package_id,
        package_sources,
    })
}

fn collect_package_sources(
    path: &Path,
    resolve: &Resolve,
    root_package_id: PackageId,
    package_source_map: &PackageSourceMap,
) -> anyhow::Result<IndexMap<PackageId, PackageSource>> {
    // Based on Resolve::push_dir:
    //
    // The deps folder may contain:
    //   $path/ deps/ my-package/*.wit: a directory that may contain multiple WIT files
    //   $path/ deps/ my-package. wit: a single-file WIT package
    //   $path/ deps/ my-package.{wasm,wat}: a wasm-encoded WIT package either in text or binary format
    //
    // Disabling "wasm" and "wat" sources could be done by disabling default features, but they are also required through other dependencies,
    // so we filter out currently not supported path patterns here, including the single file format (for now).

    let deps_dir = path.join("deps");
    let mut package_dir_paths = IndexMap::<PackageId, PackageSource>::new();
    for (package_id, package) in &resolve.packages {
        let sources = package_source_map
            .package_paths(package_id)
            .ok_or_else(|| {
                anyhow!(
                    "Failed to get package source map for package {}",
                    package.name.to_string().log_color_highlight()
                )
            })?
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();

        if package_id == root_package_id {
            package_dir_paths.insert(
                package_id,
                PackageSource {
                    dir: path.to_path_buf(),
                    files: sources,
                },
            );
        } else {
            if sources.is_empty() {
                bail!(
                    "Expected at least one source for package: {}",
                    package.name.to_string().log_color_error_highlight()
                );
            };

            let source = &sources[0];

            let extension = source.extension().ok_or_else(|| {
                anyhow!(
                    "Failed to get extension for wit source: {}",
                    source.log_color_highlight()
                )
            })?;

            if extension != "wit" {
                bail!(
                    "Only wit sources are supported, source: {}",
                    source.log_color_highlight()
                );
            }

            let parent = source.parent().ok_or_else(|| {
                anyhow!(
                    "Failed to get parent for wit source: {}",
                    source.log_color_highlight()
                )
            })?;

            if parent == deps_dir {
                bail!(
                    "Single-file wit packages without folder are not supported, source: {}",
                    source.log_color_highlight()
                );
            }

            package_dir_paths.insert(
                package_id,
                PackageSource {
                    dir: parent.to_path_buf(),
                    files: sources,
                },
            );
        }
    }
    Ok(package_dir_paths)
}

pub struct ResolvedWitComponent {
    main_package_name: PackageName,
    resolved_wit_dir: Option<ResolvedWitDir>,
    app_component_deps: HashSet<AppComponentName>,
    source_referenced_package_deps: HashSet<PackageName>,
    source_contained_package_deps: HashSet<PackageName>,
    source_component_deps: BTreeSet<AppComponentName>, // NOTE: BTree for making dep sorting deterministic
    generated_component_deps: Option<HashSet<AppComponentName>>,
}

#[derive(Default)]
enum ExtractedAgentTypes {
    #[default]
    NotAnAgent,
    ToBeExtracted,
    Extracted(Vec<AgentType>),
}

pub struct ResolvedWitApplication {
    components: BTreeMap<AppComponentName, ResolvedWitComponent>, // NOTE: BTree for making dep sorting deterministic
    package_to_component: HashMap<PackageName, AppComponentName>,
    stub_package_to_component: HashMap<PackageName, AppComponentName>,
    interface_package_to_component: HashMap<PackageName, AppComponentName>,
    component_order: Vec<AppComponentName>,
    agent_types: BTreeMap<AppComponentName, ExtractedAgentTypes>,
    enable_wasmtime_fs_cache: bool,
}

impl ResolvedWitApplication {
    pub async fn new(
        app: &Application,
        profile: Option<&BuildProfileName>,
        tools_with_ensured_common_deps: &ToolsWithEnsuredCommonDeps,
        enable_wasmtime_fs_cache: bool,
    ) -> ValidatedResult<Self> {
        log_action("Resolving", "application wit directories");
        let _indent = LogIndent::new();

        let mut resolved_app = Self {
            components: Default::default(),
            package_to_component: Default::default(),
            stub_package_to_component: Default::default(),
            interface_package_to_component: Default::default(),
            component_order: Default::default(),
            agent_types: BTreeMap::new(),
            enable_wasmtime_fs_cache,
        };

        let mut validation = ValidationBuilder::new();

        resolved_app
            .add_components_from_app(
                &mut validation,
                app,
                profile,
                tools_with_ensured_common_deps,
            )
            .await;

        resolved_app.validate_package_names(&mut validation);
        resolved_app.collect_component_deps(app, &mut validation);
        resolved_app.sort_components_by_source_deps(&mut validation);
        resolved_app.extract_agent_types(&mut validation);

        validation.build(resolved_app)
    }

    pub fn is_agent(&self, app_component_name: &AppComponentName) -> bool {
        !matches!(
            self.agent_types
                .get(app_component_name)
                .unwrap_or(&ExtractedAgentTypes::NotAnAgent),
            ExtractedAgentTypes::NotAnAgent
        )
    }

    pub async fn get_extracted_agent_types(
        &mut self,
        app_component_name: &AppComponentName,
        compiled_wasm_path: &Path,
    ) -> anyhow::Result<Vec<AgentType>> {
        match self.agent_types.get(app_component_name) {
            None => Err(anyhow!(
                "No agent information available about component: {}",
                app_component_name
            )),
            Some(ExtractedAgentTypes::NotAnAgent) => {
                Err(anyhow!("Component {} is not an agent", app_component_name))
            }
            Some(ExtractedAgentTypes::ToBeExtracted) => {
                let agent_types = crate::model::agent::extraction::extract_agent_types(
                    compiled_wasm_path,
                    self.enable_wasmtime_fs_cache,
                )
                .await?;
                self.agent_types.insert(
                    app_component_name.clone(),
                    ExtractedAgentTypes::Extracted(agent_types.clone()),
                );
                Ok(agent_types)
            }
            Some(ExtractedAgentTypes::Extracted(agent_types)) => Ok(agent_types.clone()),
        }
    }

    fn validate_package_names(&self, validation: &mut ValidationBuilder) {
        if self.package_to_component.len() != self.components.len() {
            let mut package_names_to_component_names =
                BTreeMap::<&PackageName, Vec<&AppComponentName>>::new();
            for (component_name, component) in &self.components {
                package_names_to_component_names
                    .entry(&component.main_package_name)
                    .and_modify(|component_names| component_names.push(component_name))
                    .or_insert_with(|| vec![&component_name]);
            }

            validation.add_errors(
                package_names_to_component_names,
                |(package_name, component_names)| {
                    (component_names.len() > 1).then(|| {
                        (
                            vec![
                                ("package name", package_name.to_string()),
                                (
                                    "component names",
                                    component_names.iter().map(|s| s.as_str()).join(", "),
                                ),
                            ],
                            "Same package name is used for multiple components".to_string(),
                        )
                    })
                },
            );
        }
    }

    fn add_resolved_component(
        &mut self,
        component_name: AppComponentName,
        resolved_component: ResolvedWitComponent,
    ) {
        self.package_to_component.insert(
            resolved_component.main_package_name.clone(),
            component_name.clone(),
        );
        self.stub_package_to_component.insert(
            naming::wit::client_parser_package_name(&resolved_component.main_package_name),
            component_name.clone(),
        );
        self.interface_package_to_component.insert(
            naming::wit::exports_parser_package_name(&resolved_component.main_package_name),
            component_name.clone(),
        );
        self.components.insert(component_name, resolved_component);
    }

    async fn add_components_from_app(
        &mut self,
        validation: &mut ValidationBuilder,
        app: &Application,
        profile: Option<&BuildProfileName>,
        tools_with_ensured_common_deps: &ToolsWithEnsuredCommonDeps,
    ) {
        for component_name in app.component_names() {
            validation.push_context("component name", component_name.to_string());

            let source_wit_dir = app.component_source_wit(component_name, profile);
            let generated_wit_dir = app.component_generated_wit(component_name, profile);

            let app_component_deps = app
                .component_dependencies(component_name)
                .iter()
                .filter(|&dep| dep.dep_type.is_wasm_rpc())
                .filter_map(|dep| dep.as_dependent_app_component())
                .map(|dep| dep.name)
                .collect();

            let resolved_component = if source_wit_dir.is_dir() {
                log_action(
                    "Resolving",
                    format!(
                        "component wit dirs for {} ({}, {})",
                        component_name.as_str().log_color_highlight(),
                        source_wit_dir.log_color_highlight(),
                        generated_wit_dir.log_color_highlight(),
                    ),
                );

                Self::resolve_using_wit_dir(
                    component_name,
                    &source_wit_dir,
                    &generated_wit_dir,
                    app_component_deps,
                )
            } else {
                log_action(
                    "Resolving",
                    format!(
                        "component interface using base WASM for {} ({}, {})",
                        component_name.as_str().log_color_highlight(),
                        source_wit_dir.log_color_highlight(),
                        generated_wit_dir.log_color_highlight(),
                    ),
                );

                Self::resolve_using_base_wasm(
                    component_name,
                    app_component_deps,
                    &source_wit_dir,
                    tools_with_ensured_common_deps,
                )
                .await
            };

            match resolved_component {
                Ok(resolved_component) => {
                    self.add_resolved_component(component_name.clone(), resolved_component);
                }
                Err(err) => validation.add_error(format!("{err:?}")),
            }

            validation.pop_context();
        }
    }

    fn resolve_using_wit_dir(
        component_name: &AppComponentName,
        source_wit_dir: &Path,
        generated_wit_dir: &Path,
        app_component_deps: HashSet<AppComponentName>,
    ) -> anyhow::Result<ResolvedWitComponent> {
        let unresolved_source_package_group = UnresolvedPackageGroup::parse_dir(source_wit_dir)
            .with_context(|| {
                anyhow!(
                    "Failed to parse component {} main package in source wit dir {}",
                    component_name.as_str().log_color_error_highlight(),
                    source_wit_dir.log_color_highlight(),
                )
            })?;

        let source_referenced_package_deps = unresolved_source_package_group
            .main
            .foreign_deps
            .keys()
            .cloned()
            .collect();

        let source_contained_package_deps = {
            let deps_path = source_wit_dir.join("deps");
            if !deps_path.exists() {
                HashSet::new()
            } else {
                parse_wit_deps_dir(&deps_path)?
                    .into_iter()
                    .map(|package_group| package_group.main.name)
                    .collect::<HashSet<_>>()
            }
        };

        let main_package_name = unresolved_source_package_group.main.name.clone();

        let resolved_generated_wit_dir = ResolvedWitDir::new(generated_wit_dir).ok();
        let generated_has_same_main_package_name = resolved_generated_wit_dir
            .as_ref()
            .map(|wit| wit.main_package())
            .transpose()?
            .map(|generated_main_package| main_package_name == generated_main_package.name)
            .unwrap_or_default();
        let resolved_generated_wit_dir = generated_has_same_main_package_name
            .then_some(resolved_generated_wit_dir)
            .flatten();

        Ok(ResolvedWitComponent {
            main_package_name,
            resolved_wit_dir: resolved_generated_wit_dir,
            app_component_deps,
            source_referenced_package_deps,
            source_contained_package_deps,
            source_component_deps: Default::default(),
            generated_component_deps: Default::default(),
        })
    }

    async fn resolve_using_base_wasm(
        component_name: &AppComponentName,
        app_component_deps: HashSet<AppComponentName>,
        source_wit_dir: &Path,
        tools_with_ensured_common_deps: &ToolsWithEnsuredCommonDeps,
    ) -> anyhow::Result<ResolvedWitComponent> {
        if !source_wit_dir.exists() {
            // The source WIT path does not exist. We add a special case here for paths pointing into `node_modules`
            // and trigger an `npm install` if needed. This should be generalized later as an initialization step.
            if source_wit_dir
                .components()
                .any(|c| c.as_os_str() == OsStr::new("node_modules"))
            {
                ensure_common_deps_for_tool(tools_with_ensured_common_deps, "node").await?;
            }
        }

        let source_wasm = std::fs::read(source_wit_dir)
            .with_context(|| anyhow!("Failed to read the source WIT path as a file"))?;
        let wasm = wit_parser::decoding::decode(&source_wasm).with_context(|| {
            anyhow!(
                "Failed to decode the source WIT path as a WASM component: {}",
                source_wit_dir.log_color_highlight()
            )
        })?;

        // The WASM has no root package name (always root:component with world root) so
        // we use the app component name as a main package name
        let main_package_name = component_name.to_package_name()?;

        // When using a WASM as a "source WIT", we are currently not supporting transforming
        // that into a generated WIT dir, just treat it as a static interface definition for
        // the given component. So resolved_wit_dir in this case is the resolved _source_ WIT
        // parsed from the WASM.
        let resolved_wit_dir = ResolvedWitDir::from_wasm(source_wit_dir, &wasm);

        let resolve = wasm.resolve();
        let root_package_id = wasm.package();
        let root_world_id = resolve.select_world(root_package_id, None)?;
        let root_world = resolve.worlds.get(root_world_id).ok_or_else(|| {
            anyhow!(
                "Failed to get root world from the resolved component interface: {}",
                source_wit_dir.log_color_highlight()
            )
        })?;

        let mut source_referenced_package_deps = HashSet::new();
        for import in root_world.imports.values() {
            if let WorldItem::Interface { id, .. } = import {
                let iface = resolve.interfaces.get(*id).ok_or_else(|| {
                    anyhow!("Failed to get interface from the resolved component interface")
                })?;
                if let Some(package) = iface.package {
                    let pkg = resolve.packages.get(package).ok_or_else(|| {
                        anyhow!("Failed to get package from the resolved component interface")
                    })?;
                    source_referenced_package_deps.insert(pkg.name.clone());
                }
            }
        }

        Ok(ResolvedWitComponent {
            main_package_name,
            resolved_wit_dir: Some(resolved_wit_dir),
            app_component_deps,
            source_referenced_package_deps,
            source_contained_package_deps: Default::default(),
            source_component_deps: Default::default(),
            generated_component_deps: Default::default(),
        })
    }

    fn collect_component_deps(&mut self, app: &Application, validation: &mut ValidationBuilder) {
        fn component_deps<
            'a,
            I: IntoIterator<Item = &'a PackageName>,
            O: FromIterator<AppComponentName>,
        >(
            known_package_deps: &HashMap<PackageName, AppComponentName>,
            dep_package_names: I,
        ) -> O {
            dep_package_names
                .into_iter()
                .filter_map(|package_name| known_package_deps.get(package_name).cloned())
                .collect()
        }

        let mut deps = HashMap::<
            AppComponentName,
            (
                BTreeSet<AppComponentName>,
                Option<HashSet<AppComponentName>>,
            ),
        >::new();
        for (component_name, component) in &self.components {
            deps.insert(
                component_name.clone(),
                (
                    component_deps(
                        &self.interface_package_to_component,
                        &component.source_referenced_package_deps,
                    ),
                    component.resolved_wit_dir.as_ref().map(|wit_dir| {
                        component_deps(
                            &self.stub_package_to_component,
                            wit_dir.resolve.package_names.keys(),
                        )
                    }),
                ),
            );
        }
        for (component_name, (source_deps, generated_deps)) in deps {
            let component = self.components.get_mut(&component_name).unwrap();
            component.source_component_deps = source_deps;
            component.generated_component_deps = generated_deps;
        }

        for (component_name, component) in &self.components {
            let main_deps: HashSet<AppComponentName> = component_deps(
                &self.package_to_component,
                &component.source_referenced_package_deps,
            );

            let stub_deps: HashSet<AppComponentName> = component_deps(
                &self.stub_package_to_component,
                &component.source_referenced_package_deps,
            );

            if !main_deps.is_empty() || !stub_deps.is_empty() {
                validation.push_context("component name", component_name.to_string());
                validation.push_context("package name", component.main_package_name.to_string());
                {
                    validation.push_context(
                        "source",
                        app.component_source(component_name).display().to_string(),
                    );
                }

                for dep_component_name in main_deps {
                    let dep_package_name = &self
                        .component(&dep_component_name)
                        .unwrap()
                        .main_package_name;

                    validation
                        .push_context("referenced package name", dep_package_name.to_string());

                    validation.add_error(formatdoc!("
                      Direct WIT package reference to component {} main package {} is not supported.
                      For using component clients, declare them in the app manifest as WASM-RPC dependency.
                      For using exported types from another component, use the component 'exports' package (e.g.: ns:package-name-exports).",
                      dep_component_name.to_string().log_color_highlight(),
                      dep_package_name.to_string().log_color_error_highlight()
                    ));

                    validation.pop_context();
                }

                for dep_component_name in stub_deps {
                    let dep_package_name = &self
                        .component(&dep_component_name)
                        .unwrap()
                        .main_package_name;

                    validation
                        .push_context("referenced package name", dep_package_name.to_string());

                    validation.add_error(formatdoc!("
                      Direct WIT package reference to component {} client package {} is not supported.
                      For using component clients, declare them in the app manifest as WASM-RPC dependency.
                      For using exported types from another component, use the component 'exports' package (e.g.: ns:package-name-exports).",
                      dep_component_name.to_string().log_color_highlight(),
                      dep_package_name.to_string().log_color_error_highlight()
                    ));

                    validation.pop_context();
                }

                validation.pop_context();
                validation.pop_context();
            }
        }
    }

    fn sort_components_by_source_deps(&mut self, validation: &mut ValidationBuilder) {
        struct Visit<'a> {
            resolved_app: &'a ResolvedWitApplication,
            component_names_by_id: Vec<&'a AppComponentName>,
            component_names_to_id: HashMap<&'a AppComponentName, usize>,
            visited: HashSet<usize>,
            visiting: HashSet<usize>,
            path: Vec<usize>,
            component_order: Vec<AppComponentName>,
        }

        impl<'a> Visit<'a> {
            fn new(resolved_app: &'a ResolvedWitApplication) -> Self {
                let component_names_by_id = resolved_app.components.keys().collect::<Vec<_>>();
                let component_names_to_id = component_names_by_id
                    .iter()
                    .enumerate()
                    .map(|(id, name)| (*name, id))
                    .collect();

                Self {
                    resolved_app,
                    component_names_by_id,
                    component_names_to_id,
                    visited: Default::default(),
                    visiting: Default::default(),
                    path: vec![],
                    component_order: vec![],
                }
            }

            fn visit_all(mut self) -> Result<Vec<AppComponentName>, Vec<&'a AppComponentName>> {
                for (component_id, &component_name) in
                    self.component_names_by_id.clone().iter().enumerate()
                {
                    if !self.visited.contains(&component_id)
                        && !self.visit(component_id, component_name)
                    {
                        return Err(self
                            .path
                            .into_iter()
                            .map(|component_id| self.component_names_by_id[component_id])
                            .collect::<Vec<_>>());
                    }
                }
                Ok(self.component_order)
            }

            fn visit(&mut self, component_id: usize, component_name: &AppComponentName) -> bool {
                if self.visited.contains(&component_id) {
                    true
                } else if self.visiting.contains(&component_id) {
                    self.path.push(component_id);
                    false
                } else {
                    let component = self.resolved_app.components.get(component_name).unwrap();

                    self.path.push(component_id);
                    self.visiting.insert(component_id);

                    for dep_component_name in &component.source_component_deps {
                        if !self.visit(
                            self.component_names_to_id[dep_component_name],
                            dep_component_name,
                        ) {
                            return false;
                        }
                    }

                    self.visiting.remove(&component_id);
                    self.visited.insert(component_id);
                    self.component_order.push(component_name.clone());
                    self.path.pop();

                    true
                }
            }
        }

        match Visit::new(self).visit_all() {
            Ok(component_order) => {
                self.component_order = component_order;
            }
            Err(recursive_path) => {
                validation.add_error(format!(
                    "Found component interface package use cycle: {}",
                    recursive_path
                        .iter()
                        .map(|s| s.as_str().log_color_error_highlight())
                        .join(", ")
                ));
            }
        }
    }

    fn extract_agent_types(&mut self, validation: &mut ValidationBuilder) {
        for (name, component) in &self.components {
            if let Some(resolved_wit_dir) = &component.resolved_wit_dir {
                match resolved_wit_dir.is_agent() {
                    Ok(true) => {
                        log_action(
                            "Extracting",
                            format!(
                                "agent types defined in {}",
                                name.as_str().log_color_highlight()
                            ),
                        );
                        self.agent_types
                            .insert(name.clone(), ExtractedAgentTypes::ToBeExtracted);
                    }
                    Ok(false) => {
                        log_action(
                            "Skipping",
                            format!(
                                "extraction of agent types defined in {}, not an agent",
                                name.as_str().log_color_highlight()
                            ),
                        );
                        self.agent_types
                            .insert(name.clone(), ExtractedAgentTypes::NotAnAgent);
                    }
                    Err(err) => {
                        validation.add_error(format!(
                            "Failed to check if component {} is an agent: {}",
                            name.as_str().log_color_error_highlight(),
                            err
                        ));
                    }
                }
            }
        }
    }

    pub fn component_order(&self) -> &[AppComponentName] {
        &self.component_order
    }

    pub fn component_order_cloned(&self) -> Vec<AppComponentName> {
        self.component_order.clone()
    }

    pub fn component(
        &self,
        component_name: &AppComponentName,
    ) -> Result<&ResolvedWitComponent, Error> {
        self.components.get(component_name).ok_or_else(|| {
            anyhow!(
                "Component not found: {}",
                component_name.as_str().log_color_error_highlight()
            )
        })
    }

    // NOTE: Intended to be used for non-component wit package deps, so it does not include
    //       component interface packages, as those are added from stubs
    pub fn missing_generic_source_package_deps(
        &self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<Vec<PackageName>> {
        let component = self.component(component_name)?;
        Ok(component
            .source_referenced_package_deps
            .iter()
            .filter(|&package_name| {
                !component
                    .source_contained_package_deps
                    .contains(package_name)
                    && !self
                        .interface_package_to_component
                        .contains_key(package_name)
            })
            .cloned()
            .collect::<Vec<_>>())
    }

    pub fn component_exports_package_deps(
        &self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<Vec<(PackageName, AppComponentName)>> {
        let component = self.component(component_name)?;
        Ok(component
            .source_referenced_package_deps
            .iter()
            .filter_map(|package_name| {
                match self.interface_package_to_component.get(package_name) {
                    Some(dep_component_name) if dep_component_name != component_name => {
                        Some((package_name.clone(), dep_component_name.clone()))
                    }
                    _ => None,
                }
            })
            .collect())
    }

    // NOTE: this does not mean that the dependencies themselves are up-to-date, rather
    //       only checks if there are difference in set of dependencies specified in the
    //       application model vs in wit dependencies
    pub fn is_dep_graph_up_to_date(
        &self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<bool> {
        let component = self.component(component_name)?;
        Ok(match &component.generated_component_deps {
            Some(generated_deps) => &component.app_component_deps == generated_deps,
            None => false,
        })
    }

    // NOTE: this does not mean that the dependency itself is up-to-date, rather
    //       only checks if it is present as wit package dependency
    pub fn has_as_wit_dep(
        &self,
        component_name: &AppComponentName,
        dep_component_name: &AppComponentName,
    ) -> anyhow::Result<bool> {
        let component = self.component(component_name)?;

        Ok(match &component.generated_component_deps {
            Some(generated_deps) => generated_deps.contains(dep_component_name),
            None => false,
        })
    }

    pub fn root_package_name(
        &self,
        component_name: &AppComponentName,
    ) -> anyhow::Result<PackageName> {
        let component = self.component(component_name)?;
        Ok(component.main_package_name.clone())
    }
}

pub fn parse_wit_deps_dir(path: &Path) -> Result<Vec<UnresolvedPackageGroup>, Error> {
    let mut entries = path
        .read_dir()
        .and_then(|read_dir| read_dir.collect::<std::io::Result<Vec<_>>>())
        .with_context(|| {
            anyhow!(
                "Failed to read wit dependencies from {}",
                path.log_color_error_highlight()
            )
        })?;
    entries.sort_by_key(|e| e.file_name());
    entries
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            // NOTE: unlike wit_resolve - for now - we do not support:
            //         - symlinks
            //         - single file deps
            //         - wasm or wat deps
            path.is_dir().then(|| {
                UnresolvedPackageGroup::parse_dir(&path).with_context(|| {
                    anyhow!(
                        "Failed to parse wit dependency package {}",
                        path.log_color_error_highlight()
                    )
                })
            })
        })
        .collect::<Result<Vec<_>, _>>()
}

pub struct WitDepsResolver {
    sources: Vec<PathBuf>,
    packages: HashMap<PathBuf, HashMap<PackageName, UnresolvedPackageGroup>>,
}

impl WitDepsResolver {
    pub fn new(sources: Vec<PathBuf>) -> anyhow::Result<Self> {
        // TODO: Can be removed once we fixed all docs and templates
        std::env::set_var("WIT_REQUIRE_F32_F64", "0");

        let mut packages = HashMap::<PathBuf, HashMap<PackageName, UnresolvedPackageGroup>>::new();

        for source in &sources {
            packages.insert(
                source.to_path_buf().clone(),
                parse_wit_deps_dir(source)?
                    .into_iter()
                    .map(|package| (package.main.name.clone(), package))
                    .collect(),
            );
        }

        Ok(Self { sources, packages })
    }

    pub fn package(&self, package_name: &PackageName) -> anyhow::Result<&UnresolvedPackageGroup> {
        for source in &self.sources {
            if let Some(package) = self.packages.get(source).unwrap().get(package_name) {
                return Ok(package);
            }
        }
        bail!(
            "Package {} not found, wit source directories searched: {}",
            package_name.to_string().log_color_error_highlight(),
            if self.sources.is_empty() {
                "no wit source directories were provided".to_string()
            } else {
                let sources = self
                    .sources
                    .iter()
                    .map(|s| s.log_color_highlight())
                    .join(", ");

                // TODO: add fuzzy search tips, once app is extracted from stubgen
                let packages = &self
                    .sources
                    .iter()
                    .flat_map(|source| self.packages.get(source).unwrap().keys())
                    .collect::<BTreeSet<_>>();

                let hint = {
                    match packages.iter().find(|pn| {
                        pn.namespace == package_name.namespace && pn.name == package_name.name
                    }) {
                        Some(package_name_match) => {
                            format!(
                                "Did you mean {}?\n\n",
                                package_name_match.to_string().log_color_highlight()
                            )
                        }
                        None => "".to_string(),
                    }
                };

                let packages = packages
                    .iter()
                    .map(|package_name| format!("- {package_name}"))
                    .join("\n");

                if !packages.is_empty() {
                    format!(
                        "{}\n\n{}{}:\n{}",
                        sources,
                        hint,
                        "Available packages".log_color_help_group(),
                        packages
                    )
                } else {
                    sources
                }
            }
        )
    }

    pub fn package_sources(
        &self,
        package_name: &PackageName,
    ) -> anyhow::Result<impl Iterator<Item = &Path>> {
        self.package(package_name)
            .map(|package| package.source_map.source_files())
    }

    pub fn package_names_with_transitive_deps(
        &self,
        packages: &[PackageName],
    ) -> anyhow::Result<HashSet<PackageName>> {
        fn visit(
            resolver: &WitDepsResolver,
            all_package_names: &mut HashSet<PackageName>,
            package_name: &PackageName,
        ) -> anyhow::Result<()> {
            if !all_package_names.contains(package_name) {
                let package = resolver.package(package_name)?;
                all_package_names.insert(package_name.clone());
                for package_name in package.main.foreign_deps.keys() {
                    visit(resolver, all_package_names, package_name)?;
                }
            }

            Ok(())
        }

        let mut all_package_names = HashSet::<PackageName>::new();
        for package_name in packages {
            visit(self, &mut all_package_names, package_name)?;
        }
        Ok(all_package_names)
    }

    pub fn add_packages_with_transitive_deps_to_wit_dir(
        &self,
        packages: &[PackageName],
        target_deps_dir: &Path,
    ) -> anyhow::Result<()> {
        for package_name in self.package_names_with_transitive_deps(packages)? {
            log_action(
                "Adding",
                format!(
                    "package dependency {} to {}",
                    package_name.to_string().log_color_highlight(),
                    target_deps_dir.log_color_highlight(),
                ),
            );
            let _indent = LogIndent::new();

            for source in self.package_sources(&package_name)? {
                let source = PathExtra::new(source);
                let source_parent = PathExtra::new(source.parent()?);

                let target = target_deps_dir
                    .join(naming::wit::DEPS_DIR)
                    .join(source_parent.file_name_to_string()?)
                    .join(source.file_name_to_string()?);

                log_action(
                    "Copying",
                    format!(
                        "package dependency source from {} to {}",
                        source.log_color_highlight(),
                        target.log_color_highlight()
                    ),
                );
                fs::copy(source, target)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{ExportedFunction, ResolvedWitDir};
    use test_r::test;

    #[test]
    fn test_exported_functions() {
        let resolved =
            ResolvedWitDir::new(&PathBuf::from("test-data/wit/many-ways-to-export")).unwrap();
        let mut exports = resolved.exported_functions().unwrap();
        exports.sort();
        assert_eq!(
            exports,
            vec![
                ExportedFunction::Interface {
                    interface_name: "test:exports/iface1".to_string(),
                    function_name: "func2".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:exports/iface6".to_string(),
                    function_name: "func6".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:exports/iface8".to_string(),
                    function_name: "func8".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:exports/iface9".to_string(),
                    function_name: "func9".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:sub/iface12".to_string(),
                    function_name: "func12".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:sub/iface13".to_string(),
                    function_name: "func13".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:sub/iface4".to_string(),
                    function_name: "func5".to_string()
                },
                ExportedFunction::Interface {
                    interface_name: "test:sub2/iface10".to_string(),
                    function_name: "func10".to_string()
                },
                ExportedFunction::InlineInterface {
                    export_name: "inline-iface".to_string(),
                    function_name: "func4".to_string()
                },
                ExportedFunction::InlineFunction {
                    world_name: "api".to_string(),
                    function_name: "func1".to_string()
                },
                ExportedFunction::InlineFunction {
                    world_name: "api".to_string(),
                    function_name: "func2".to_string()
                }
            ]
        )
    }
}
