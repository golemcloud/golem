use crate::fs::PathExtra;
use crate::log::{log_action, LogColorize, LogIndent};
use crate::model::app::{Application, ComponentName, ComponentPropertiesExtensions, ProfileName};
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::{fs, naming};
use anyhow::{anyhow, bail, Context, Error};
use indexmap::IndexMap;
use indoc::formatdoc;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use wit_parser::{
    Package, PackageId, PackageName, PackageSourceMap, Resolve, UnresolvedPackageGroup,
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
    pub fn new(path: &Path) -> anyhow::Result<ResolvedWitDir> {
        resolve_wit_dir(path)
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
}

fn resolve_wit_dir(path: &Path) -> anyhow::Result<ResolvedWitDir> {
    // TODO: Can be removed once we fixed all docs and examples
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
    resolved_generated_wit_dir: Option<ResolvedWitDir>,
    app_component_deps: HashSet<ComponentName>,
    source_referenced_package_deps: HashSet<PackageName>,
    source_contained_package_deps: HashSet<PackageName>,
    source_component_deps: BTreeSet<ComponentName>, // NOTE: BTree for making dep sorting deterministic
    generated_component_deps: Option<HashSet<ComponentName>>,
}

pub struct ResolvedWitApplication {
    components: BTreeMap<ComponentName, ResolvedWitComponent>, // NOTE: BTree for making dep sorting deterministic
    package_to_component: HashMap<PackageName, ComponentName>,
    stub_package_to_component: HashMap<PackageName, ComponentName>,
    interface_package_to_component: HashMap<PackageName, ComponentName>,
    component_order: Vec<ComponentName>,
}

impl ResolvedWitApplication {
    pub fn new<CPE: ComponentPropertiesExtensions>(
        app: &Application<CPE>,
        profile: Option<&ProfileName>,
    ) -> ValidatedResult<Self> {
        // TODO: Can be removed once we fixed all docs and examples
        std::env::set_var("WIT_REQUIRE_F32_F64", "0");

        log_action("Resolving", "application wit directories");
        let _indent = LogIndent::new();

        let mut resolved_app = Self {
            components: Default::default(),
            package_to_component: Default::default(),
            stub_package_to_component: Default::default(),
            interface_package_to_component: Default::default(),
            component_order: Default::default(),
        };

        let mut validation = ValidationBuilder::new();

        resolved_app.add_components_from_app(&mut validation, app, profile);

        resolved_app.validate_package_names(&mut validation);
        resolved_app.collect_component_deps(app, &mut validation);
        resolved_app.sort_components_by_source_deps(&mut validation);

        validation.build(resolved_app)
    }

    fn validate_package_names(&self, validation: &mut ValidationBuilder) {
        if self.package_to_component.len() != self.components.len() {
            let mut package_names_to_component_names =
                BTreeMap::<&PackageName, Vec<&ComponentName>>::new();
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
        component_name: ComponentName,
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

    fn add_components_from_app<CPE: ComponentPropertiesExtensions>(
        &mut self,
        validation: &mut ValidationBuilder,
        app: &Application<CPE>,
        profile: Option<&ProfileName>,
    ) {
        for component_name in app.component_names() {
            validation.push_context("component name", component_name.to_string());

            let source_wit_dir = app.component_source_wit(component_name, profile);
            let generated_wit_dir = app.component_generated_wit(component_name, profile);

            log_action(
                "Resolving",
                format!(
                    "component wit dirs for {} ({}, {})",
                    component_name.as_str().log_color_highlight(),
                    source_wit_dir.log_color_highlight(),
                    generated_wit_dir.log_color_highlight(),
                ),
            );

            let resolved_component = (|| -> anyhow::Result<ResolvedWitComponent> {
                let unresolved_source_package_group =
                    UnresolvedPackageGroup::parse_dir(&source_wit_dir).with_context(|| {
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
                    let deps_path =
                        Path::new(&app.component_properties(component_name, profile).source_wit)
                            .join("deps");
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

                let resolved_generated_wit_dir = ResolvedWitDir::new(&generated_wit_dir).ok();
                let generated_has_same_main_package_name = resolved_generated_wit_dir
                    .as_ref()
                    .map(|wit| wit.main_package())
                    .transpose()?
                    .map(|generated_main_package| main_package_name == generated_main_package.name)
                    .unwrap_or_default();
                let resolved_generated_wit_dir = generated_has_same_main_package_name
                    .then_some(resolved_generated_wit_dir)
                    .flatten();

                let app_component_deps = app
                    .component_wasm_rpc_dependencies(component_name)
                    .iter()
                    .cloned()
                    .map(|dep| dep.name)
                    .collect();

                Ok(ResolvedWitComponent {
                    main_package_name,
                    resolved_generated_wit_dir,
                    app_component_deps,
                    source_referenced_package_deps,
                    source_contained_package_deps,
                    source_component_deps: Default::default(),
                    generated_component_deps: Default::default(),
                })
            })();

            match resolved_component {
                Ok(resolved_component) => {
                    self.add_resolved_component(component_name.clone(), resolved_component);
                }
                Err(err) => validation.add_error(format!("{:?}", err)),
            }

            validation.pop_context();
        }
    }

    fn collect_component_deps<CPE: ComponentPropertiesExtensions>(
        &mut self,
        app: &Application<CPE>,
        validation: &mut ValidationBuilder,
    ) {
        fn component_deps<
            'a,
            I: IntoIterator<Item = &'a PackageName>,
            O: FromIterator<ComponentName>,
        >(
            known_package_deps: &HashMap<PackageName, ComponentName>,
            dep_package_names: I,
        ) -> O {
            dep_package_names
                .into_iter()
                .filter_map(|package_name| known_package_deps.get(package_name).cloned())
                .collect()
        }

        let mut deps = HashMap::<
            ComponentName,
            (BTreeSet<ComponentName>, Option<HashSet<ComponentName>>),
        >::new();
        for (component_name, component) in &self.components {
            deps.insert(
                component_name.clone(),
                (
                    component_deps(
                        &self.interface_package_to_component,
                        &component.source_referenced_package_deps,
                    ),
                    component
                        .resolved_generated_wit_dir
                        .as_ref()
                        .map(|wit_dir| {
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
            let main_deps: HashSet<ComponentName> = component_deps(
                &self.package_to_component,
                &component.source_referenced_package_deps,
            );

            let stub_deps: HashSet<ComponentName> = component_deps(
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
            component_names_by_id: Vec<&'a ComponentName>,
            component_names_to_id: HashMap<&'a ComponentName, usize>,
            visited: HashSet<usize>,
            visiting: HashSet<usize>,
            path: Vec<usize>,
            component_order: Vec<ComponentName>,
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

            fn visit_all(mut self) -> Result<Vec<ComponentName>, Vec<&'a ComponentName>> {
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

            fn visit(&mut self, component_id: usize, component_name: &ComponentName) -> bool {
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

    pub fn component_order(&self) -> &[ComponentName] {
        &self.component_order
    }

    pub fn component_order_cloned(&self) -> Vec<ComponentName> {
        self.component_order.clone()
    }

    fn component(&self, component_name: &ComponentName) -> Result<&ResolvedWitComponent, Error> {
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
        component_name: &ComponentName,
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
        component_name: &ComponentName,
    ) -> anyhow::Result<Vec<(PackageName, ComponentName)>> {
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
    pub fn is_dep_graph_up_to_date(&self, component_name: &ComponentName) -> anyhow::Result<bool> {
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
        component_name: &ComponentName,
        dep_component_name: &ComponentName,
    ) -> anyhow::Result<bool> {
        let component = self.component(component_name)?;

        Ok(match &component.generated_component_deps {
            Some(generated_deps) => generated_deps.contains(dep_component_name),
            None => false,
        })
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
        // TODO: Can be removed once we fixed all docs and examples
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
            "Package {} not found, sources searched: {}",
            package_name.to_string().log_color_error_highlight(),
            if self.sources.is_empty() {
                "no sources were provided".to_string()
            } else {
                self.sources
                    .iter()
                    .map(|s| s.log_color_highlight())
                    .join(", ")
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
