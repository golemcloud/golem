use crate::commands::log::{log_action, LogColorize, LogIndent};
use crate::model::validation::{ValidatedResult, ValidationBuilder};
use crate::model::wasm_rpc::Application;
use crate::naming;
use anyhow::{anyhow, bail, Context};
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
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
    pub app_component_deps: HashSet<String>,
    pub input_referenced_package_deps: HashSet<PackageName>,
    pub input_contained_package_deps: HashSet<PackageName>,
    pub input_component_deps: HashSet<String>,
    pub output_component_deps: Option<HashSet<String>>,
}

pub struct ResolvedWitApplication {
    pub components: HashMap<String, ResolvedWitComponent>,
    pub package_to_component: HashMap<PackageName, String>,
    pub stub_package_to_component: HashMap<PackageName, String>,
    pub interface_package_to_component: HashMap<PackageName, String>,
}

impl ResolvedWitApplication {
    pub fn new(app: &Application) -> ValidatedResult<Self> {
        let mut resolved_app = Self {
            components: Default::default(),
            package_to_component: Default::default(),
            stub_package_to_component: Default::default(),
            interface_package_to_component: Default::default(),
        };

        let mut validation = ValidationBuilder::new();

        resolved_app.add_components_from_app(&mut validation, app);

        // TODO: validate conflicting package names

        if !validation.has_any_errors() {
            resolved_app.collect_component_deps();
        }

        validation.build(resolved_app)
    }

    fn add_resolved_component(
        &mut self,
        component_name: String,
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
            validation.push_context("component name", component_name.clone());

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
                        let mut entries = deps_path
                            .read_dir()
                            .and_then(|read_dir| read_dir.collect::<std::io::Result<Vec<_>>>())
                            .with_context(|| {
                                anyhow!(
                                    "Failed to read component {} wit dependencies from {}",
                                    component_name,
                                    deps_path.display(),
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
                                    UnresolvedPackageGroup::parse_dir(&path)
                                        .with_context(|| {
                                            anyhow!(
                                                "Failed to parse component {} wit dependency {}",
                                                component_name,
                                                path.display()
                                            )
                                        })
                                        .map(|package_group| package_group.main.name)
                                })
                            })
                            .collect::<Result<HashSet<_>, _>>()?
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

    pub fn collect_component_deps(&mut self) {
        fn component_deps<'a, I: IntoIterator<Item = &'a PackageName>>(
            known_package_deps: &HashMap<PackageName, String>,
            dep_package_names: I,
        ) -> HashSet<String> {
            dep_package_names
                .into_iter()
                .filter_map(|package_name| known_package_deps.get(package_name).cloned())
                .collect()
        }

        let mut deps = HashMap::<String, (HashSet<String>, Option<HashSet<String>>)>::new();
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

    // NOTE: this does not mean that the dependencies themselves are up-to-date, rather
    //       only checks if there are difference in set of dependencies specified in the
    //       application model vs in wit dependencies
    pub fn is_dep_graph_up_to_date(&self, component_name: &String) -> bool {
        match self.components.get(component_name) {
            Some(component) => match &component.output_component_deps {
                Some(output_deps) => &component.app_component_deps == output_deps,
                None => false,
            },
            None => false,
        }
    }

    // NOTE: this does not mean that the dependency itself is up-to-date, rather
    //       only checks if it is present as wit package dependency
    pub fn has_as_wit_dep(&self, component_name: &String, dep_component_name: &String) -> bool {
        match self.components.get(component_name) {
            Some(component) => match &component.output_component_deps {
                Some(output_deps) => output_deps.contains(dep_component_name),
                None => false,
            },
            None => false,
        }
    }
}
