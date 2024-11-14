use crate::commands::log::{log_action, LogColorize, LogIndent};
use crate::model::wasm_rpc::{Application, ComponentName};
use crate::naming;
use crate::validation::{ValidatedResult, ValidationBuilder};
use anyhow::{anyhow, bail, Context, Error};
use indexmap::IndexMap;
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
                self.path.display()
            )
        })
    }

    pub fn package_sources(&self, package_id: PackageId) -> Result<&PackageSource, Error> {
        self.package_sources.get(&package_id).with_context(|| {
            anyhow!(
                "Failed to get package sources by id: {:?}, wit dir: {}",
                package_id,
                self.path.display()
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
        .with_context(|| anyhow!("Failed to resolve wit dir: {}", path.display()))?;

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
                    package.name
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
                bail!("Expected at least one source for package: {}", package.name);
            };

            let source = &sources[0];

            let extension = source.extension().ok_or_else(|| {
                anyhow!(
                    "Failed to get extension for wit source: {}",
                    source.display()
                )
            })?;

            if extension != "wit" {
                bail!(
                    "Only wit sources are supported, source: {}",
                    source.display()
                );
            }

            let parent = source.parent().ok_or_else(|| {
                anyhow!("Failed to get parent for wit source: {}", source.display())
            })?;

            if parent == deps_dir {
                bail!(
                    "Single-file wit packages without folder are not supported, source: {}",
                    source.display()
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
    pub main_package_name: PackageName,
    pub unresolved_input_package_group: UnresolvedPackageGroup,
    pub resolved_output_wit_dir: Option<ResolvedWitDir>,
    pub app_component_deps: HashSet<ComponentName>,
    pub input_referenced_package_deps: HashSet<PackageName>,
    pub input_contained_package_deps: HashSet<PackageName>,
    pub input_component_deps: BTreeSet<ComponentName>, // NOTE: BTree for making dep sorting deterministic
    pub output_component_deps: Option<HashSet<ComponentName>>,
}

pub struct ResolvedWitApplication {
    pub components: BTreeMap<ComponentName, ResolvedWitComponent>, // NOTE: BTree for making dep sorting deterministic
    pub package_to_component: HashMap<PackageName, ComponentName>,
    pub stub_package_to_component: HashMap<PackageName, ComponentName>,
    pub interface_package_to_component: HashMap<PackageName, ComponentName>,
    pub component_order: Vec<ComponentName>,
}

impl ResolvedWitApplication {
    pub fn new(app: &Application) -> ValidatedResult<Self> {
        let mut resolved_app = Self {
            components: Default::default(),
            package_to_component: Default::default(),
            stub_package_to_component: Default::default(),
            interface_package_to_component: Default::default(),
            component_order: Default::default(),
        };

        let mut validation = ValidationBuilder::new();

        resolved_app.add_components_from_app(&mut validation, app);

        // TODO: validate conflicting package names

        if !validation.has_any_errors() {
            resolved_app.collect_component_deps();
            resolved_app.sort_components_by_input_deps(&mut validation);
        }

        validation.build(resolved_app)
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
            naming::wit::stub_package_name(&resolved_component.main_package_name),
            component_name.clone(),
        );
        self.interface_package_to_component.insert(
            naming::wit::interface_parser_package_name(&resolved_component.main_package_name),
            component_name.clone(),
        );
        self.components.insert(component_name, resolved_component);
    }

    fn add_components_from_app(&mut self, validation: &mut ValidationBuilder, app: &Application) {
        log_action("Resolving", "application wit directories");
        let _indent = LogIndent::new();

        for (component_name, component) in &app.wasm_components_by_name {
            validation.push_context("component name", component_name.to_string());

            let input_wit = app.component_input_wit(component_name);
            let output_wit = app.component_output_wit(component_name);

            log_action(
                "Resolving",
                format!(
                    "component wit dirs for {} ({}, {})",
                    component_name.log_color_highlight(),
                    input_wit.log_color_highlight(),
                    output_wit.log_color_highlight(),
                ),
            );

            let resolved_component = (|| -> anyhow::Result<ResolvedWitComponent> {
                let unresolved_input_package_group = UnresolvedPackageGroup::parse_dir(&input_wit)
                    .with_context(|| {
                        anyhow!(
                            "Failed to parse component {} main package in input wit dir {}",
                            component_name,
                            input_wit.display()
                        )
                    })?;

                let input_referenced_package_deps = unresolved_input_package_group
                    .main
                    .foreign_deps
                    .keys()
                    .cloned()
                    .collect();

                let input_contained_package_deps = {
                    let deps_path = component.input_wit.join("deps");
                    if !deps_path.exists() {
                        HashSet::new()
                    } else {
                        parse_wit_deps_dir(&deps_path)?
                            .into_iter()
                            .map(|package_group| package_group.main.name)
                            .collect::<HashSet<_>>()
                    }
                };

                let main_package_name = unresolved_input_package_group.main.name.clone();

                let resolved_output_wit_dir = ResolvedWitDir::new(&output_wit).ok();
                let output_has_same_main_package_name = resolved_output_wit_dir
                    .as_ref()
                    .map(|wit| wit.main_package())
                    .transpose()?
                    .map(|output_main_package| main_package_name == output_main_package.name)
                    .unwrap_or_default();
                let resolved_output_wit_dir = output_has_same_main_package_name
                    .then_some(resolved_output_wit_dir)
                    .flatten();

                let app_component_deps = component.wasm_rpc_dependencies.iter().cloned().collect();

                Ok(ResolvedWitComponent {
                    main_package_name,
                    unresolved_input_package_group,
                    resolved_output_wit_dir,
                    app_component_deps,
                    input_referenced_package_deps,
                    input_contained_package_deps,
                    input_component_deps: Default::default(),
                    output_component_deps: Default::default(),
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

    fn collect_component_deps(&mut self) {
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
                        &component.input_referenced_package_deps,
                    ),
                    component.resolved_output_wit_dir.as_ref().map(|wit_dir| {
                        component_deps(
                            &self.stub_package_to_component,
                            wit_dir.resolve.package_names.keys(),
                        )
                    }),
                ),
            );
        }
        for (component_name, (input_deps, output_deps)) in deps {
            let component = self.components.get_mut(&component_name).unwrap();
            component.input_component_deps = input_deps;
            component.output_component_deps = output_deps;
        }
    }

    fn sort_components_by_input_deps(&mut self, validation: &mut ValidationBuilder) {
        let mut component_order = Vec::with_capacity(self.components.len());

        // TODO: use some id instead of Strings?
        let mut visited = HashSet::<ComponentName>::new();
        let mut visiting = HashSet::<ComponentName>::new();

        fn visit(
            resolved_app: &ResolvedWitApplication,
            visited: &mut HashSet<ComponentName>,
            visiting: &mut HashSet<ComponentName>,
            component_order: &mut Vec<ComponentName>,
            component_name: &ComponentName,
            input_deps: &BTreeSet<ComponentName>,
        ) -> bool {
            if visited.contains(component_name) {
                true
            } else if visiting.contains(component_name) {
                false
            } else {
                visiting.insert(component_name.clone());

                for dep_component_name in input_deps {
                    if !visit(
                        resolved_app,
                        visited,
                        visiting,
                        component_order,
                        dep_component_name,
                        &resolved_app
                            .components
                            .get(dep_component_name)
                            .unwrap()
                            .input_component_deps,
                    ) {
                        return false;
                    }
                }

                visiting.remove(component_name);
                visited.insert(component_name.clone());

                component_order.push(component_name.clone());

                true
            }
        }

        for (component_name, component) in &self.components {
            if !visited.contains(component_name)
                && !visit(
                    self,
                    &mut visited,
                    &mut visiting,
                    &mut component_order,
                    component_name,
                    &component.input_component_deps,
                )
            {
                // TODO: better error message, collect full path
                component_order.push(component_name.clone());
                validation.add_error(format!(
                    "Found component cycle: {}",
                    component_order.iter().map(|s| s.as_str()).join(", ")
                ));
                break;
            }
        }

        self.component_order = component_order;
    }

    fn component(&self, component_name: &ComponentName) -> Result<&ResolvedWitComponent, Error> {
        self.components
            .get(component_name)
            .ok_or_else(|| anyhow!("Component not found: {}", component_name))
    }

    // NOTE: Intended to be used for non-component wit package deps, so it does not include
    //       component interface packages, as those are added from stubs
    pub fn missing_generic_input_package_deps(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<Vec<PackageName>> {
        let component = self.component(component_name)?;
        Ok(component
            .input_referenced_package_deps
            .iter()
            .filter(|&package_name| {
                !component
                    .input_contained_package_deps
                    .contains(package_name)
                    && !self
                        .interface_package_to_component
                        .contains_key(package_name)
            })
            .cloned()
            .collect::<Vec<_>>())
    }

    pub fn component_interface_package_deps(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<Vec<(PackageName, ComponentName)>> {
        let component = self.component(component_name)?;
        Ok(component
            .input_referenced_package_deps
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
        Ok(match &component.output_component_deps {
            Some(output_deps) => &component.app_component_deps == output_deps,
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

        Ok(match &component.output_component_deps {
            Some(output_deps) => output_deps.contains(dep_component_name),
            None => false,
        })
    }
}

pub fn parse_wit_deps_dir(path: &Path) -> Result<Vec<UnresolvedPackageGroup>, Error> {
    let mut entries = path
        .read_dir()
        .and_then(|read_dir| read_dir.collect::<std::io::Result<Vec<_>>>())
        .with_context(|| anyhow!("Failed to read wit dependencies from {}", path.display(),))?;
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
                    anyhow!("Failed to parse wit dependency package {}", path.display())
                })
            })
        })
        .collect::<Result<Vec<_>, _>>()
}
