use crate::log::LogColorize;
use crate::model::app_raw;
use crate::model::template::Template;
use crate::naming::wit::package_dep_dir_name_from_parser;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::{fs, naming};
use heck::{
    ToKebabCase, ToLowerCamelCase, ToPascalCase, ToShoutyKebabCase, ToShoutySnakeCase, ToSnakeCase,
    ToTitleCase, ToTrainCase, ToUpperCamelCase,
};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Formatter;
use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::path::{Path, PathBuf};
use wit_parser::PackageName;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[clap(rename_all = "kebab_case")]
pub enum AppBuildStep {
    GenRpc,
    Componentize,
    LinkRpc,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentName(String);

impl ComponentName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ComponentName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ComponentName {
    fn from(value: String) -> Self {
        ComponentName(value)
    }
}

impl From<&str> for ComponentName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProfileName(String);

impl ProfileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ProfileName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ProfileName {
    fn from(value: String) -> Self {
        ProfileName(value)
    }
}

impl From<&str> for ProfileName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TemplateName(String);

impl TemplateName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for TemplateName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TemplateName {
    fn from(value: String) -> Self {
        TemplateName(value)
    }
}

impl From<&str> for TemplateName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

pub fn includes_from_yaml_file(source: &Path) -> Vec<String> {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| app_raw::Application::from_yaml_str(source.as_str()).ok())
        .map(|app| {
            if app.includes.is_empty() {
                vec!["**/golem.yaml".to_string()]
            } else {
                app.includes
            }
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub enum ResolvedComponentProperties<CPE: ComponentPropertiesExtensions> {
    Properties {
        template_name: Option<TemplateName>,
        any_template_overrides: bool,
        properties: ComponentProperties<CPE>,
    },
    Profiles {
        template_name: Option<TemplateName>,
        any_template_overrides: HashMap<ProfileName, bool>,
        default_profile: ProfileName,
        profiles: HashMap<ProfileName, ComponentProperties<CPE>>,
    },
}

pub struct ComponentEffectivePropertySource<'a> {
    pub template_name: Option<&'a TemplateName>,
    pub profile: Option<&'a ProfileName>,
    pub is_requested_profile: bool,
    pub any_template_overrides: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DependencyType {
    /// Dynamic (stubless) wasm-rpc
    DynamicWasmRpc,
    /// Static (composed with compiled stub) wasm-rpc
    StaticWasmRpc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DependentComponent {
    pub name: ComponentName,
    pub dep_type: DependencyType,
}

impl PartialOrd for DependentComponent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DependentComponent {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

#[derive(Clone, Debug)]
pub struct Application<CPE: ComponentPropertiesExtensions> {
    temp_dir: Option<String>,
    wit_deps: Vec<String>,
    components: BTreeMap<ComponentName, Component<CPE>>,
    dependencies: BTreeMap<ComponentName, BTreeSet<DependentComponent>>,
    no_dependencies: BTreeSet<DependentComponent>,
}

impl<CPE: ComponentPropertiesExtensions> Application<CPE> {
    const STATIC_WASM_RPC: &'static str = "static-wasm-rpc";
    const WASM_RPC: &'static str = "wasm-rpc";

    pub fn from_raw_apps(apps: Vec<app_raw::ApplicationWithSource>) -> ValidatedResult<Self> {
        let mut validation = ValidationBuilder::new();

        let mut include = Vec::<String>::new();
        let mut include_sources = Vec::<PathBuf>::new();

        let mut temp_dir: Option<String> = None;
        let mut temp_dir_sources = Vec::<PathBuf>::new();

        let mut wit_deps = Vec::<String>::new();
        let mut wit_deps_sources = Vec::<PathBuf>::new();

        let mut templates = HashMap::<TemplateName, app_raw::ComponentTemplate>::new();
        let mut template_sources = HashMap::<TemplateName, Vec<PathBuf>>::new();

        let mut dependencies = BTreeMap::<ComponentName, BTreeSet<DependentComponent>>::new();
        let mut dependency_sources =
            HashMap::<ComponentName, HashMap<ComponentName, Vec<PathBuf>>>::new();

        let mut components = HashMap::<ComponentName, (PathBuf, app_raw::Component)>::new();
        let mut component_sources = HashMap::<ComponentName, Vec<PathBuf>>::new();

        for app in apps {
            validation.push_context("source", app.source.to_string_lossy().to_string());

            if let Some(dir) = app.application.temp_dir {
                temp_dir_sources.push(app.source.to_path_buf());
                if temp_dir.is_none() {
                    temp_dir = Some(dir);
                }
            }

            if !app.application.includes.is_empty() {
                include_sources.push(app.source.to_path_buf());
                if include.is_empty() {
                    include = app.application.includes;
                }
            }

            if !app.application.wit_deps.is_empty() {
                wit_deps_sources.push(app.source.to_path_buf());
                if wit_deps.is_empty() {
                    wit_deps = app.application.wit_deps; // TODO: resolve from source?
                }
            }

            for (template_name, template) in app.application.templates {
                validation.push_context("template", template_name.clone());

                let mut invalid_template = false;
                if template.profiles.is_empty() {
                    if template.default_profile.is_some() {
                        validation.add_error(format!(
                            "When {} is not defined then {} should not be defined",
                            "profiles".log_color_highlight(),
                            "defaultProfile".log_color_highlight()
                        ));
                        invalid_template = true;
                    }
                } else {
                    let defined_property_names =
                        template.component_properties.defined_property_names();
                    if !defined_property_names.is_empty() {
                        for property_name in defined_property_names {
                            validation.add_error(format!(
                                "When {} is defined then {} should not be defined",
                                "profiles".log_color_highlight(),
                                property_name.log_color_highlight()
                            ));
                            invalid_template = true;
                        }
                    }

                    if template.default_profile.is_none() {
                        validation.add_error(format!(
                            "When {} is defined then {} is mandatory",
                            "profiles".log_color_highlight(),
                            "defaultProfile".log_color_highlight()
                        ));
                        invalid_template = true;
                    }
                }

                let template_name = TemplateName::from(template_name);
                if template_sources.contains_key(&template_name) {
                    template_sources
                        .get_mut(&template_name)
                        .unwrap()
                        .push(app.source.to_path_buf());
                } else {
                    template_sources.insert(template_name.clone(), vec![app.source.to_path_buf()]);
                }
                if !templates.contains_key(&template_name) && !invalid_template {
                    templates.insert(template_name, template);
                }

                validation.pop_context();
            }

            for (component_name, component) in app.application.components {
                let component_name = ComponentName::from(component_name);

                if !component_sources.contains_key(&component_name) {
                    component_sources.insert(component_name.clone(), Vec::new());
                }
                component_sources
                    .get_mut(&component_name)
                    .unwrap()
                    .push(app.source.to_path_buf());

                components.insert(component_name, (app.source.to_path_buf(), component));
            }

            for (component_name, component_dependencies) in app.application.dependencies {
                let component_name = ComponentName::from(component_name);
                validation.push_context("component", component_name.to_string());

                for dependency in component_dependencies {
                    if dependency.type_ == Self::STATIC_WASM_RPC
                        || dependency.type_ == Self::WASM_RPC
                    {
                        match dependency.target {
                            Some(target) => {
                                let target_component_name = ComponentName::from(target);

                                if !dependencies.contains_key(&component_name) {
                                    dependencies.insert(component_name.clone(), BTreeSet::new());
                                }

                                let dep_type = if dependency.type_ == Self::STATIC_WASM_RPC {
                                    DependencyType::StaticWasmRpc
                                } else {
                                    DependencyType::DynamicWasmRpc
                                };

                                dependencies.get_mut(&component_name).unwrap().insert(
                                    DependentComponent {
                                        name: target_component_name.clone(),
                                        dep_type,
                                    },
                                );

                                if !dependency_sources.contains_key(&component_name) {
                                    dependency_sources
                                        .insert(component_name.clone(), HashMap::new());
                                }
                                let dependency_sources =
                                    dependency_sources.get_mut(&component_name).unwrap();
                                if !dependency_sources.contains_key(&target_component_name) {
                                    dependency_sources
                                        .insert(target_component_name.clone(), Vec::new());
                                }
                                dependency_sources
                                    .get_mut(&target_component_name)
                                    .unwrap()
                                    .push(app.source.to_path_buf());
                            }
                            None => validation.add_error(format!(
                                "Missing {} field for component wasm-rpc dependency",
                                "target".log_color_error_highlight()
                            )),
                        }
                    } else {
                        validation.add_error(format!(
                            "Unknown component dependency type: {}",
                            dependency.type_.log_color_error_highlight()
                        ));
                    }
                }

                validation.pop_context();
            }

            validation.pop_context();
        }

        for (property_name, sources) in [
            ("include", include_sources),
            ("tempDir", temp_dir_sources),
            ("witDeps", wit_deps_sources),
        ] {
            if sources.len() > 1 {
                validation.add_error(format!(
                    "Property {} is defined in multiple sources: {}",
                    property_name.log_color_highlight(),
                    sources
                        .into_iter()
                        .map(|s| s.log_color_highlight())
                        .join(", ")
                ))
            }
        }

        let non_unique_templates = template_sources
            .into_iter()
            .filter(|(_, sources)| sources.len() > 1);

        validation.add_errors(non_unique_templates, |(template_name, sources)| {
            Some((
                vec![],
                format!(
                    "Template {} defined multiple times in sources: {}",
                    template_name.as_str().log_color_highlight(),
                    sources
                        .into_iter()
                        .map(|s| s.log_color_highlight())
                        .join(", ")
                ),
            ))
        });

        let non_unique_components = component_sources
            .into_iter()
            .filter(|(_, sources)| sources.len() > 1);

        validation.add_errors(non_unique_components, |(template_name, sources)| {
            Some((
                vec![],
                format!(
                    "Component {} defined multiple times in sources: {}",
                    template_name.as_str().log_color_highlight(),
                    sources
                        .into_iter()
                        .map(|s| s.log_color_highlight())
                        .join(", ")
                ),
            ))
        });

        for (component_name, dependency_sources) in dependency_sources {
            for (target_component_name, dependency_sources) in dependency_sources {
                if dependency_sources.len() > 1 {
                    validation.push_context("component", component_name.to_string());
                    validation.push_context("target", target_component_name.to_string());

                    validation.add_warn(format!(
                        "WASM-RPC dependency is defined multiple times, sources: {}",
                        dependency_sources
                            .into_iter()
                            .map(|s| s.log_color_highlight())
                            .join(", ")
                    ));

                    validation.pop_context();
                    validation.pop_context();
                }
            }
        }

        let components = {
            let template_env = Self::template_env();

            let mut resolved_components = BTreeMap::<ComponentName, Component<CPE>>::new();
            for (component_name, (source, mut component)) in components {
                validation.push_context("source", source.to_string_lossy().to_string());
                validation.push_context("component", component_name.to_string());

                let template_with_name = match component.template {
                    Some(template_name) => {
                        let template_name = TemplateName::from(template_name);
                        match templates.get(&template_name) {
                            Some(template) => Some(Some((template_name, template))),
                            None => {
                                validation.add_error(format!(
                                    "Component references unknown template: {}",
                                    template_name.as_str().log_color_error_highlight()
                                ));
                                None
                            }
                        }
                    }
                    None => Some(None),
                };

                if let Some(template_with_name) = template_with_name {
                    let component_properties = match template_with_name {
                        Some((template_name, template)) => {
                            let mut incompatible_overrides = false;

                            let defined_property_names =
                                component.component_properties.defined_property_names();

                            if !template.profiles.is_empty() && !defined_property_names.is_empty() {
                                incompatible_overrides = true;
                                for property_name in defined_property_names {
                                    validation.add_error(
                                        format!(
                                            "Property {} cannot be used, as the component uses a template with profiles",
                                            property_name.log_color_highlight()
                                        )
                                    );
                                }
                            }

                            for profile_name in component.profiles.keys() {
                                if !template.profiles.contains_key(profile_name) {
                                    incompatible_overrides = true;
                                    validation.add_error(
                                        format!(
                                            "Profile {} cannot be used, as the component uses template {} with the following profiles: {}",
                                            profile_name.log_color_highlight(),
                                            template_name.as_str().log_color_highlight(),
                                            template.profiles.keys().map(|s| s.log_color_highlight()).join(", ")
                                        )
                                    );
                                }
                            }

                            if incompatible_overrides {
                                None
                            } else {
                                let template_context = minijinja::context! { componentName => component_name.as_str() };

                                if template.profiles.is_empty() {
                                    let rendered_template_properties =
                                        ComponentProperties::from_raw_template(
                                            &template_env,
                                            &template_context,
                                            &template.component_properties,
                                        );

                                    match rendered_template_properties {
                                        Ok(rendered_template_properties) => {
                                            match rendered_template_properties.merge_with_overrides(
                                                &source,
                                                &mut validation,
                                                component.component_properties,
                                            ) {
                                                Ok((Some(properties), any_template_overrides)) => {
                                                    Some(ResolvedComponentProperties::Properties {
                                                        template_name: Some(template_name),
                                                        any_template_overrides,
                                                        properties,
                                                    })
                                                }
                                                Ok((None, _any_template_overrides)) => None,
                                                Err(err) => {
                                                    validation.add_error(format!(
                                                        "Failed to override template {}, error: {}",
                                                        template_name
                                                            .as_str()
                                                            .log_color_highlight(),
                                                        err.to_string().log_color_error_highlight()
                                                    ));
                                                    None
                                                }
                                            }
                                        }
                                        Err(err) => {
                                            validation.add_error(format!(
                                                "Failed to render template {}, error: {}",
                                                template_name.as_str().log_color_highlight(),
                                                err.to_string().log_color_error_highlight()
                                            ));
                                            None
                                        }
                                    }
                                } else {
                                    let mut any_template_overrides =
                                        HashMap::<ProfileName, bool>::new();
                                    let mut profiles =
                                        HashMap::<ProfileName, ComponentProperties<CPE>>::new();
                                    let mut any_template_error = false;

                                    for (profile_name, template_component_properties) in
                                        &template.profiles
                                    {
                                        let rendered_template_properties =
                                            ComponentProperties::from_raw_template(
                                                &template_env,
                                                &template_context,
                                                template_component_properties,
                                            );
                                        match rendered_template_properties {
                                            Ok(rendered_template_properties) => {
                                                let properties_with_overrides = {
                                                    if let Some(component_properties) =
                                                        component.profiles.remove(profile_name)
                                                    {
                                                        rendered_template_properties
                                                            .merge_with_overrides(
                                                                &source,
                                                                &mut validation,
                                                                component_properties,
                                                            )
                                                    } else {
                                                        Ok((
                                                            Some(rendered_template_properties),
                                                            false,
                                                        ))
                                                    }
                                                };

                                                match properties_with_overrides {
                                                    Ok((Some(properties), any_overrides)) => {
                                                        any_template_overrides.insert(
                                                            profile_name.clone().into(),
                                                            any_overrides,
                                                        );
                                                        profiles.insert(
                                                            profile_name.clone().into(),
                                                            properties,
                                                        );
                                                    }
                                                    Ok((None, _any_template_overrides)) => {
                                                        any_template_error = true;
                                                    }
                                                    Err(err) => {
                                                        validation.add_error(format!(
                                                            "Failed to override template {}, error: {}",
                                                            template_name
                                                                .as_str()
                                                                .log_color_highlight(),
                                                            err.to_string().log_color_error_highlight()
                                                        ));
                                                        any_template_error = true;
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                validation.add_error(format!(
                                                    "Failed to render template {}, error: {}",
                                                    template_name.as_str().log_color_highlight(),
                                                    err.to_string().log_color_error_highlight()
                                                ));
                                                any_template_error = true
                                            }
                                        }
                                    }

                                    (!any_template_error).then(|| {
                                        ResolvedComponentProperties::Profiles {
                                            template_name: Some(template_name),
                                            any_template_overrides,
                                            default_profile: template
                                                .default_profile
                                                .clone()
                                                .expect("Missing template default profile")
                                                .into(),
                                            profiles,
                                        }
                                    })
                                }
                            }
                        }
                        None => {
                            if component.profiles.is_empty() {
                                if component.default_profile.is_some() {
                                    validation.add_error(format!(
                                        "When {} is not defined then {} should not be defined",
                                        "profiles".log_color_highlight(),
                                        "defaultProfile".log_color_highlight()
                                    ));
                                    None
                                } else {
                                    let properties = ComponentProperties::<CPE>::from_raw(
                                        component.component_properties,
                                    );

                                    match properties {
                                        Ok(properties) => {
                                            Some(ResolvedComponentProperties::Properties {
                                                template_name: None,
                                                any_template_overrides: false,
                                                properties,
                                            })
                                        }
                                        Err(err) => {
                                            validation.add_error(format!("{:?}", err));
                                            None
                                        }
                                    }
                                }
                            } else if component.default_profile.is_none() {
                                validation.add_error(format!(
                                    "When {} is defined then {} is mandatory",
                                    "profiles".log_color_highlight(),
                                    "defaultProfile".log_color_highlight()
                                ));
                                None
                            } else {
                                Some(ResolvedComponentProperties::Profiles {
                                    template_name: None,
                                    any_template_overrides: component
                                        .profiles
                                        .keys()
                                        .map(|profile_name| {
                                            (ProfileName::from(profile_name.clone()), false)
                                        })
                                        .collect(),
                                    default_profile: component.default_profile.unwrap().into(),
                                    profiles: component
                                        .profiles
                                        .into_iter()
                                        .filter_map(|(profile_name, properties)| {
                                            match ComponentProperties::<CPE>::from_raw(properties) {
                                                Ok(properties) => Some((
                                                    ProfileName::from(profile_name),
                                                    properties,
                                                )),
                                                Err(err) => {
                                                    validation.add_error(format!("{:?}", err));
                                                    None
                                                }
                                            }
                                        })
                                        .collect(),
                                })
                            }
                        }
                    };

                    if let Some(mut properties) = component_properties {
                        fn validate_properties_and_convert_extensions<
                            CPE: ComponentPropertiesExtensions,
                        >(
                            source: &Path,
                            validation: &mut ValidationBuilder,
                            properties: &mut ComponentProperties<CPE>,
                        ) -> bool {
                            let mut any_error = false;

                            for (name, value) in [
                                ("sourceWit", &properties.source_wit),
                                ("generatedWit", &properties.generated_wit),
                                ("componentWasm", &properties.component_wasm),
                            ] {
                                if value.is_empty() {
                                    validation.add_error(format!(
                                        "Property {} is empty or undefined",
                                        name.log_color_highlight()
                                    ));
                                    any_error = true;
                                }
                            }

                            let reserved_commands = BTreeSet::from(["build", "clean"]);

                            for custom_command in properties.custom_commands.keys() {
                                if reserved_commands.contains(custom_command.as_str()) {
                                    validation.add_error(format!("Cannot use {} as custom command name, reserved command names: {}",
                                                                 custom_command.log_color_error_highlight(),
                                                                 reserved_commands.iter().map(|s| s.log_color_highlight()).join(", ")
                                    ));
                                }
                            }

                            properties.extensions = CPE::convert_and_validate(
                                source,
                                validation,
                                properties.extensions_raw.take().unwrap(),
                            );
                            any_error |= properties.extensions.is_none();

                            any_error
                        }

                        let any_error: bool = match &mut properties {
                            ResolvedComponentProperties::Properties {
                                template_name,
                                any_template_overrides,
                                properties,
                            } => {
                                template_name.iter().for_each(|template_name| {
                                    validation.push_context("template", template_name.to_string());
                                    validation.push_context(
                                        "overrides",
                                        any_template_overrides.to_string(),
                                    );
                                });

                                let any_error = validate_properties_and_convert_extensions(
                                    &source,
                                    &mut validation,
                                    properties,
                                );

                                if template_name.is_some() {
                                    validation.pop_context();
                                    validation.pop_context();
                                }

                                any_error
                            }
                            ResolvedComponentProperties::Profiles {
                                template_name,
                                any_template_overrides,
                                profiles,
                                ..
                            } => {
                                template_name.iter().for_each(|template_name| {
                                    validation.push_context("template", template_name.to_string());
                                });

                                let mut any_error = false;

                                for (profile_name, properties) in profiles {
                                    validation.push_context("profile", profile_name.to_string());
                                    let any_template_overrides =
                                        any_template_overrides.get(profile_name);
                                    any_template_overrides.iter().for_each(
                                        |any_template_overrides| {
                                            validation.push_context(
                                                "overrides",
                                                any_template_overrides.to_string(),
                                            );
                                        },
                                    );

                                    any_error |= validate_properties_and_convert_extensions(
                                        &source,
                                        &mut validation,
                                        properties,
                                    );

                                    if any_template_overrides.is_some() {
                                        validation.pop_context();
                                    }
                                    validation.pop_context();
                                }

                                if template_name.is_some() {
                                    validation.pop_context();
                                }

                                any_error
                            }
                        };

                        if !any_error {
                            resolved_components
                                .insert(component_name.clone(), Component { source, properties });
                        }
                    }
                }

                validation.pop_context();
                validation.pop_context();
            }

            resolved_components
        };

        validation.build(Self {
            temp_dir,
            wit_deps,
            components,
            dependencies,
            no_dependencies: BTreeSet::new(),
        })
    }

    fn template_env<'a>() -> minijinja::Environment<'a> {
        let mut env = minijinja::Environment::new();

        env.add_filter("to_snake_case", |str: &str| str.to_snake_case());

        env.add_filter("to_kebab_case", |str: &str| str.to_kebab_case());
        env.add_filter("to_lower_camel_case", |str: &str| str.to_lower_camel_case());
        env.add_filter("to_pascal_case", |str: &str| str.to_pascal_case());
        env.add_filter("to_shouty_kebab_case", |str: &str| {
            str.to_shouty_kebab_case()
        });
        env.add_filter("to_shouty_snake_case", |str: &str| {
            str.to_shouty_snake_case()
        });
        env.add_filter("to_snake_case", |str: &str| str.to_snake_case());
        env.add_filter("to_title_case", |str: &str| str.to_title_case());
        env.add_filter("to_train_case", |str: &str| str.to_train_case());
        env.add_filter("to_upper_camel_case", |str: &str| str.to_upper_camel_case());

        env
    }

    pub fn component_names(&self) -> impl Iterator<Item = &ComponentName> {
        self.components.keys()
    }

    pub fn wit_deps(&self) -> Vec<PathBuf> {
        self.wit_deps.iter().map(PathBuf::from).collect()
    }

    pub fn all_wasm_rpc_dependencies(&self) -> BTreeSet<DependentComponent> {
        self.dependencies.values().flatten().cloned().collect()
    }

    pub fn all_profiles(&self) -> BTreeSet<ProfileName> {
        self.component_names()
            .flat_map(|component_name| self.component_profiles(component_name))
            .collect()
    }

    pub fn all_option_profiles(&self) -> BTreeSet<Option<ProfileName>> {
        let mut profiles = self
            .component_names()
            .flat_map(|component_name| self.component_profiles(component_name))
            .map(Some)
            .collect::<BTreeSet<_>>();
        profiles.insert(None);
        profiles
    }

    pub fn all_custom_commands(&self, profile: Option<&ProfileName>) -> BTreeSet<String> {
        self.component_names()
            .flat_map(|component_name| {
                self.component_properties(component_name, profile)
                    .custom_commands
                    .keys()
                    .cloned()
            })
            .collect()
    }

    pub fn temp_dir(&self) -> &Path {
        match self.temp_dir.as_ref() {
            Some(temp_dir) => Path::new(temp_dir),
            None => Path::new("golem-temp"),
        }
    }

    pub fn task_result_marker_dir(&self) -> PathBuf {
        self.temp_dir().join("task_results")
    }

    fn component(&self, component_name: &ComponentName) -> &Component<CPE> {
        self.components
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_source(&self, component_name: &ComponentName) -> &Path {
        &self.component(component_name).source
    }

    pub fn component_source_dir(&self, component_name: &ComponentName) -> &Path {
        self.component(component_name).source_dir()
    }

    pub fn component_wasm_rpc_dependencies(
        &self,
        component_name: &ComponentName,
    ) -> &BTreeSet<DependentComponent> {
        self.dependencies
            .get(component_name)
            .unwrap_or(&self.no_dependencies)
    }

    fn component_profiles(&self, component_name: &ComponentName) -> HashSet<ProfileName> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties { .. } => HashSet::new(),
            ResolvedComponentProperties::Profiles { profiles, .. } => {
                profiles.keys().cloned().collect()
            }
        }
    }

    pub fn component_effective_property_source<'a>(
        &'a self,
        component_name: &ComponentName,
        profile: Option<&'a ProfileName>,
    ) -> ComponentEffectivePropertySource<'a> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties {
                template_name,
                any_template_overrides,
                ..
            } => ComponentEffectivePropertySource {
                template_name: template_name.as_ref(),
                profile: None,
                is_requested_profile: false,
                any_template_overrides: *any_template_overrides,
            },
            ResolvedComponentProperties::Profiles {
                template_name,
                any_template_overrides,
                default_profile,
                profiles,
            } => {
                let effective_profile = profile
                    .map(|profile| {
                        if profiles.contains_key(profile) {
                            profile
                        } else {
                            default_profile
                        }
                    })
                    .unwrap_or_else(|| default_profile);

                let is_requested_profile = Some(&effective_profile) == profile.as_ref();

                let any_template_overrides = any_template_overrides
                    .get(effective_profile)
                    .cloned()
                    .unwrap_or_default();
                ComponentEffectivePropertySource {
                    template_name: template_name.as_ref(),
                    profile: Some(effective_profile),
                    is_requested_profile,
                    any_template_overrides,
                }
            }
        }
    }

    pub fn component_properties(
        &self,
        component_name: &ComponentName,
        profile: Option<&ProfileName>,
    ) -> &ComponentProperties<CPE> {
        match &self.component(component_name).properties {
            ResolvedComponentProperties::Properties { properties, .. } => properties,
            ResolvedComponentProperties::Profiles {
                default_profile,
                profiles,
                ..
            } => profiles
                .get(
                    profile
                        .map(|profile| {
                            if profiles.contains_key(profile) {
                                profile
                            } else {
                                default_profile
                            }
                        })
                        .unwrap_or_else(|| default_profile),
                )
                .unwrap(),
        }
    }

    pub fn component_source_wit(
        &self,
        component_name: &ComponentName,
        profile: Option<&ProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .source_wit
                .clone(),
        )
    }

    pub fn component_generated_base_wit(&self, component_name: &ComponentName) -> PathBuf {
        self.temp_dir()
            .join("generated-base-wit")
            .join(component_name.as_str())
    }

    pub fn component_generated_base_wit_interface_package_dir(
        &self,
        component_name: &ComponentName,
        interface_package_name: &PackageName,
    ) -> PathBuf {
        self.component_generated_base_wit(component_name)
            .join(naming::wit::DEPS_DIR)
            .join(package_dep_dir_name_from_parser(interface_package_name))
            .join(naming::wit::INTERFACE_WIT_FILE_NAME)
    }

    pub fn component_generated_wit(
        &self,
        component_name: &ComponentName,
        profile: Option<&ProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .generated_wit
                .clone(),
        )
    }

    pub fn component_wasm(
        &self,
        component_name: &ComponentName,
        profile: Option<&ProfileName>,
    ) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name, profile)
                .component_wasm
                .clone(),
        )
    }

    pub fn component_linked_wasm(
        &self,
        component_name: &ComponentName,
        profile: Option<&ProfileName>,
    ) -> PathBuf {
        self.component_source_dir(component_name).join(
            self.component_properties(component_name, profile)
                .linked_wasm
                .as_ref()
                .cloned()
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    self.temp_dir()
                        .join("linked-wasm")
                        .join(format!("{}.wasm", component_name.as_str()))
                }),
        )
    }

    fn stub_build_dir(&self) -> PathBuf {
        self.temp_dir().join("stub")
    }

    pub fn stub_temp_build_dir(&self, component_name: &ComponentName) -> PathBuf {
        self.stub_build_dir()
            .join(component_name.as_str())
            .join("temp-build")
    }

    pub fn stub_wasm(&self, component_name: &ComponentName) -> PathBuf {
        self.stub_build_dir()
            .join(component_name.as_str())
            .join("stub.wasm")
    }

    pub fn stub_wit(&self, component_name: &ComponentName) -> PathBuf {
        self.stub_build_dir()
            .join(component_name.as_str())
            .join(naming::wit::WIT_DIR)
    }
}

#[derive(Clone, Debug)]
pub struct Component<CPE: ComponentPropertiesExtensions> {
    pub source: PathBuf,
    pub properties: ResolvedComponentProperties<CPE>,
}

impl<CPE: ComponentPropertiesExtensions> Component<CPE> {
    pub fn source_dir(&self) -> &Path {
        let parent = self.source.parent().unwrap_or_else(|| {
            panic!(
                "Failed to get parent for component, source: {}",
                self.source.display()
            )
        });
        if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComponentProperties<CPE: ComponentPropertiesExtensions> {
    pub source_wit: String,
    pub generated_wit: String,
    pub component_wasm: String,
    pub linked_wasm: Option<String>,
    pub build: Vec<app_raw::ExternalCommand>,
    pub custom_commands: HashMap<String, Vec<app_raw::ExternalCommand>>,
    pub clean: Vec<String>,

    // TODO: clean up: move extensions_raw to a temporary var and make extensions non optional
    pub extensions_raw: Option<CPE::Raw>,
    pub extensions: Option<CPE>,
}

impl<CPE: ComponentPropertiesExtensions> ComponentProperties<CPE> {
    fn from_raw(raw: app_raw::ComponentProperties) -> anyhow::Result<Self> {
        Ok(Self {
            source_wit: raw.source_wit.unwrap_or_default(),
            generated_wit: raw.generated_wit.unwrap_or_default(),
            component_wasm: raw.component_wasm.unwrap_or_default(),
            linked_wasm: raw.linked_wasm,
            build: raw.build,
            custom_commands: raw.custom_commands,
            clean: raw.clean,
            extensions_raw: Some(CPE::raw_from_serde_json(serde_json::Value::Object(
                raw.extensions,
            ))?),
            extensions: None,
        })
    }

    fn from_raw_template<C: Serialize>(
        env: &minijinja::Environment,
        ctx: &C,
        template_properties: &app_raw::ComponentProperties,
    ) -> anyhow::Result<Self> {
        ComponentProperties::from_raw(template_properties.render(env, ctx)?)
    }

    fn merge_with_overrides(
        mut self,
        source: &Path,
        validation: &mut ValidationBuilder,
        overrides: app_raw::ComponentProperties,
    ) -> anyhow::Result<(Option<Self>, bool)> {
        let mut any_overrides = false;

        if let Some(source_wit) = overrides.source_wit {
            self.source_wit = source_wit;
            any_overrides = true;
        }

        if let Some(generated_wit) = overrides.generated_wit {
            self.generated_wit = generated_wit;
            any_overrides = true;
        }

        if let Some(component_wasm) = overrides.component_wasm {
            self.component_wasm = component_wasm;
            any_overrides = true;
        }

        if overrides.linked_wasm.is_some() {
            self.linked_wasm = overrides.linked_wasm;
            any_overrides = true;
        }

        if !overrides.build.is_empty() {
            self.build = overrides.build;
            any_overrides = true;
        }

        for (custom_command_name, custom_command) in overrides.custom_commands {
            if self.custom_commands.contains_key(&custom_command_name) {
                any_overrides = true;
            }
            self.custom_commands
                .insert(custom_command_name, custom_command);
        }

        let any_extension_error = {
            if !overrides.extensions.is_empty() {
                let extensions_override =
                    CPE::raw_from_serde_json(serde_json::Value::Object(overrides.extensions))?;
                let (extensions, any_extension_overrides) = self
                    .extensions
                    .take()
                    .unwrap_or_default()
                    .merge_wit_overrides(source, validation, extensions_override)?;

                any_overrides |= any_overrides || any_extension_overrides;

                match extensions {
                    Some(extensions) => {
                        self.extensions = Some(extensions);
                        false
                    }
                    None => true,
                }
            } else {
                false
            }
        };

        if any_extension_error {
            Ok((None, false))
        } else {
            Ok((Some(self), any_overrides))
        }
    }
}

pub trait ComponentPropertiesExtensions: Sized + Debug + Clone + Default {
    type Raw: Debug + Clone;

    fn raw_from_serde_json(extensions: serde_json::Value) -> serde_json::Result<Self::Raw>;

    fn convert_and_validate(
        source: &Path,
        validation: &mut ValidationBuilder,
        raw: Self::Raw,
    ) -> Option<Self>;

    fn merge_wit_overrides(
        self,
        source: &Path,
        validation: &mut ValidationBuilder,
        overrides: Self::Raw,
    ) -> serde_json::Result<(Option<Self>, bool)>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentPropertiesExtensionsNone {}

impl ComponentPropertiesExtensions for ComponentPropertiesExtensionsNone {
    type Raw = Self;

    fn raw_from_serde_json(extensions: serde_json::Value) -> serde_json::Result<Self::Raw>
    where
        Self: Sized,
    {
        serde_json::from_value(extensions)
    }

    fn convert_and_validate(
        _source: &Path,
        _validation: &mut ValidationBuilder,
        raw: Self::Raw,
    ) -> Option<Self> {
        Some(raw)
    }

    fn merge_wit_overrides(
        self,
        _source: &Path,
        _validation: &mut ValidationBuilder,
        _overrides: Self::Raw,
    ) -> serde_json::Result<(Option<Self>, bool)> {
        Ok((Some(self), false))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComponentPropertiesExtensionsAny;

impl ComponentPropertiesExtensions for ComponentPropertiesExtensionsAny {
    type Raw = Self;

    fn raw_from_serde_json(_extensions: serde_json::Value) -> serde_json::Result<Self>
    where
        Self: Sized,
    {
        Ok(ComponentPropertiesExtensionsAny)
    }

    fn convert_and_validate(
        _source: &Path,
        _validation: &mut ValidationBuilder,
        raw: Self::Raw,
    ) -> Option<Self::Raw> {
        Some(raw)
    }

    fn merge_wit_overrides(
        self,
        _source: &Path,
        _validation: &mut ValidationBuilder,
        _overrides: Self::Raw,
    ) -> serde_json::Result<(Option<Self>, bool)> {
        Ok((Some(self), false))
    }
}
