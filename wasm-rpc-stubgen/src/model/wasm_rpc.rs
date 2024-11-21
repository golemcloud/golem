use crate::log::LogColorize;
use crate::model::oam;
use crate::model::oam::TypedTraitProperties;
use crate::model::unknown_properties::HasUnknownProperties;
use crate::model::wasm_rpc::template::Template;
use crate::naming::wit::package_dep_dir_name_from_parser;
use crate::validation::{ValidatedResult, ValidationBuilder};
use crate::{fs, naming};
use golem_wasm_rpc::WASM_RPC_VERSION;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use wit_parser::PackageName;

pub const DEFAULT_CONFIG_FILE_NAME: &str = "golem.yaml";

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
        ComponentName(value.to_string())
    }
}

mod raw {
    use crate::model::oam;
    use crate::model::unknown_properties::{HasUnknownProperties, UnknownProperties};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    pub const OAM_TRAIT_TYPE_WASM_RPC: &str = "wasm-rpc";

    pub const OAM_COMPONENT_TYPE_WASM: &str = "wasm";
    pub const OAM_COMPONENT_TYPE_WASM_BUILD: &str = "wasm-build";
    pub const OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD: &str = "wasm-rpc-stub-build";

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct BuildStep {
        pub command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub dir: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub inputs: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub outputs: Vec<String>,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WasmComponentProperties {
        pub component_template: Option<String>,
        pub input_wit: Option<String>,
        pub output_wit: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub build: Vec<BuildStep>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        pub build_profiles: HashMap<String, Vec<BuildStep>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default_build_profile: Option<String>,
        pub input_wasm: Option<String>,
        pub output_wasm: Option<String>,
        #[serde(flatten)]
        pub unknown_properties: UnknownProperties,
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WasmComponentTemplateProperties {
        pub input_wit: String,
        pub output_wit: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub build: Vec<BuildStep>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        pub build_profiles: HashMap<String, Vec<BuildStep>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub default_build_profile: Option<String>,
        pub input_wasm: String,
        pub output_wasm: String,
        #[serde(flatten)]
        pub unknown_properties: UnknownProperties,
    }

    impl HasUnknownProperties for WasmComponentProperties {
        fn unknown_properties(&self) -> &UnknownProperties {
            &self.unknown_properties
        }
    }

    impl oam::TypedComponentProperties for WasmComponentProperties {
        fn component_type() -> &'static str {
            OAM_COMPONENT_TYPE_WASM
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    pub struct WasmRpcTraitProperties {
        #[serde(rename = "componentName")]
        pub component_name: String,
        #[serde(flatten)]
        pub unknown_properties: UnknownProperties,
    }

    impl HasUnknownProperties for WasmRpcTraitProperties {
        fn unknown_properties(&self) -> &UnknownProperties {
            &self.unknown_properties
        }
    }

    impl oam::TypedTraitProperties for WasmRpcTraitProperties {
        fn trait_type() -> &'static str {
            OAM_TRAIT_TYPE_WASM_RPC
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ComponentBuildProperties {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub include: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub build_dir: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub wit_deps: Vec<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        pub component_templates: HashMap<String, WasmComponentTemplateProperties>,
        #[serde(flatten)]
        pub unknown_properties: UnknownProperties,
    }

    impl oam::TypedComponentProperties for ComponentBuildProperties {
        fn component_type() -> &'static str {
            OAM_COMPONENT_TYPE_WASM_BUILD
        }
    }

    impl HasUnknownProperties for ComponentBuildProperties {
        fn unknown_properties(&self) -> &UnknownProperties {
            &self.unknown_properties
        }
    }

    #[derive(Clone, Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct ComponentStubBuildProperties {
        pub component_name: Option<String>,
        pub build_dir: Option<String>,
        pub wasm: Option<String>,
        pub wit: Option<String>,
        pub world: Option<String>,
        pub crate_version: Option<String>,
        pub wasm_rpc_path: Option<String>,
        pub wasm_rpc_version: Option<String>,
        #[serde(flatten)]
        pub unknown_properties: UnknownProperties,
    }

    impl oam::TypedComponentProperties for ComponentStubBuildProperties {
        fn component_type() -> &'static str {
            OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD
        }
    }

    impl HasUnknownProperties for ComponentStubBuildProperties {
        fn unknown_properties(&self) -> &UnknownProperties {
            &self.unknown_properties
        }
    }
}

mod template {
    use crate::model::wasm_rpc::raw::{BuildStep, WasmComponentTemplateProperties};
    use crate::model::wasm_rpc::WasmComponentProperties;
    use serde::Serialize;
    use std::collections::HashMap;

    pub trait Template<C: Serialize> {
        type Rendered;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error>;
    }

    impl<C: Serialize> Template<C> for String {
        type Rendered = String;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            env.render_str(self, ctx)
        }
    }

    impl<C: Serialize, T: Template<C>> Template<C> for Option<T> {
        type Rendered = Option<T::Rendered>;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            match self {
                Some(template) => Ok(Some(template.render(env, ctx)?)),
                None => Ok(None),
            }
        }
    }

    impl<C: Serialize, T: Template<C>> Template<C> for Vec<T> {
        type Rendered = Vec<T::Rendered>;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            self.iter().map(|elem| elem.render(env, ctx)).collect()
        }
    }

    impl<C: Serialize, T: Template<C>> Template<C> for HashMap<String, T> {
        type Rendered = HashMap<String, T::Rendered>;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            let mut rendered = HashMap::<String, T::Rendered>::new();
            for (key, template) in self {
                rendered.insert(key.clone(), template.render(env, ctx)?);
            }
            Ok(rendered)
        }
    }

    impl<C: Serialize> Template<C> for BuildStep {
        type Rendered = BuildStep;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            Ok(BuildStep {
                command: self.command.render(env, ctx)?,
                dir: self.dir.render(env, ctx)?,
                inputs: self.inputs.render(env, ctx)?,
                outputs: self.outputs.render(env, ctx)?,
            })
        }
    }

    impl<C: Serialize> Template<C> for WasmComponentTemplateProperties {
        type Rendered = WasmComponentProperties;

        fn render(
            &self,
            env: &minijinja::Environment,
            ctx: &C,
        ) -> Result<Self::Rendered, minijinja::Error> {
            Ok(WasmComponentProperties {
                build: self.build.render(env, ctx)?,
                build_profiles: self.build_profiles.render(env, ctx)?,
                default_build_profile: self.default_build_profile.render(env, ctx)?,
                input_wit: self.input_wit.render(env, ctx)?.into(),
                output_wit: self.output_wit.render(env, ctx)?.into(),
                input_wasm: self.input_wasm.render(env, ctx)?.into(),
                output_wasm: self.output_wasm.render(env, ctx)?.into(),
            })
        }
    }
}

pub fn init_oam_app(_component_name: ComponentName) -> oam::Application {
    // TODO: let's do it as part of https://github.com/golemcloud/wasm-rpc/issues/89
    todo!()
}

// This a lenient non-validating peek for the include build property,
// as that is used early, during source collection
pub fn include_glob_patter_from_yaml_file(source: &Path) -> Option<String> {
    fs::read_to_string(source)
        .ok()
        .and_then(|source| oam::Application::from_yaml_str(source.as_str()).ok())
        .and_then(|mut oam_app| {
            let mut includes = oam_app
                .spec
                .extract_components_by_type(&BTreeSet::from([raw::OAM_COMPONENT_TYPE_WASM_BUILD]))
                .remove(raw::OAM_COMPONENT_TYPE_WASM_BUILD)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|component| {
                    component
                        .typed_properties::<raw::ComponentBuildProperties>()
                        .ok()
                        .and_then(|properties| properties.include)
                });

            match includes.next() {
                Some(include) => {
                    // Only return it if it's unique (if not it will cause validation errors later)
                    includes.next().is_none().then_some(include)
                }
                None => None,
            }
        })
}

#[derive(Clone, Debug)]
pub struct Application {
    common_wasm_build: Option<WasmBuild>,
    common_wasm_rpc_stub_build: Option<WasmRpcStubBuild>,
    wasm_rpc_stub_builds_by_name: BTreeMap<ComponentName, WasmRpcStubBuild>,
    wasm_components_by_name: BTreeMap<ComponentName, WasmComponent>,
    wasm_component_rendered_templates_by_name: BTreeMap<ComponentName, WasmComponentProperties>,
}

impl Application {
    pub fn from_oam_apps(oam_apps: Vec<oam::ApplicationWithSource>) -> ValidatedResult<Self> {
        let mut validation = ValidationBuilder::new();

        let (all_components, all_wasm_builds, all_wasm_rpc_stub_builds) = {
            let mut all_components = Vec::<WasmComponent>::new();
            let mut all_wasm_builds = Vec::<WasmBuild>::new();
            let mut all_wasm_rpc_stub_builds = Vec::<WasmRpcStubBuild>::new();

            for mut oam_app in oam_apps {
                let (components, wasm_build, wasm_rpc_stub_build) =
                    Self::extract_and_convert_oam_components(&mut validation, &mut oam_app);
                all_components.extend(components);
                all_wasm_builds.extend(wasm_build);
                all_wasm_rpc_stub_builds.extend(wasm_rpc_stub_build);
            }

            (all_components, all_wasm_builds, all_wasm_rpc_stub_builds)
        };

        let wasm_components_by_name = Self::validate_components(&mut validation, all_components);

        let (common_wasm_rpc_stub_build, wasm_rpc_stub_builds_by_name) =
            Self::validate_wasm_rpc_stub_builds(
                &mut validation,
                &wasm_components_by_name,
                all_wasm_rpc_stub_builds,
            );

        let common_wasm_build = Self::validate_wasm_builds(&mut validation, all_wasm_builds);

        let wasm_component_rendered_templates_by_name = {
            let env = minijinja::Environment::new();

            wasm_components_by_name
                .iter()
                .filter_map(|(component_name, component)| {
                    if let WasmComponentPropertySource::Template { template_name } =
                        &component.properties
                    {
                        Some((component_name, template_name))
                    } else {
                        None
                    }
                })
                .filter_map(|(component_name, template_name)| {
                    let templates = common_wasm_build
                        .as_ref()
                        .map(|build| &build.component_templates);

                    match templates.and_then(|templates| templates.get(template_name)) {
                        Some(template) => {
                            match template.render(
                                &env,
                                &minijinja::context! { componentName => component_name.as_str() },
                            ) {
                                Ok(properties) => Some((component_name.clone(), properties)),
                                Err(err) => {
                                    validation
                                        .push_context("component_name", component_name.to_string());
                                    validation
                                        .push_context("template_name", template_name.to_string());

                                    validation.add_error(format!(
                                        "Failed to render component template: {}",
                                        err
                                    ));
                                    validation.pop_context();
                                    validation.pop_context();

                                    None
                                }
                            }
                        }
                        None => {
                            validation.add_error(format!(
                                "Component template {} not found, {}",
                                template_name.log_color_error_highlight(),
                                match templates {
                                    Some(templates) if !templates.is_empty() => {
                                        format!(
                                            "available templates: {}",
                                            templates
                                                .keys()
                                                .map(|s| s.log_color_highlight())
                                                .join(", ")
                                        )
                                    }
                                    _ => {
                                        "no templates are defined".to_string()
                                    }
                                }
                            ));
                            validation.pop_context();
                            None
                        }
                    }
                })
                .collect()
        };

        validation.build(Self {
            common_wasm_build,
            common_wasm_rpc_stub_build,
            wasm_rpc_stub_builds_by_name,
            wasm_components_by_name,
            wasm_component_rendered_templates_by_name,
        })
    }

    fn extract_and_convert_oam_components(
        validation: &mut ValidationBuilder,
        oam_app: &mut oam::ApplicationWithSource,
    ) -> (Vec<WasmComponent>, Vec<WasmBuild>, Vec<WasmRpcStubBuild>) {
        validation.push_context("source", oam_app.source_as_string());

        let mut components_by_type =
            oam_app
                .application
                .spec
                .extract_components_by_type(&BTreeSet::from([
                    raw::OAM_COMPONENT_TYPE_WASM,
                    raw::OAM_COMPONENT_TYPE_WASM_BUILD,
                    raw::OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD,
                ]));

        let wasm_components = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            raw::OAM_COMPONENT_TYPE_WASM,
            Self::convert_wasm_component,
        );

        let wasm_builds = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            raw::OAM_COMPONENT_TYPE_WASM_BUILD,
            Self::convert_wasm_build,
        );

        let wasm_rpc_stub_builds = Self::convert_components(
            &oam_app.source,
            validation,
            &mut components_by_type,
            raw::OAM_COMPONENT_TYPE_WASM_RPC_STUB_BUILD,
            Self::convert_wasm_rpc_stub_build,
        );

        validation.add_warns(&oam_app.application.spec.components, |component| {
            Some((
                vec![
                    ("component name", component.name.clone()),
                    ("component type", component.component_type.clone()),
                ],
                "Unknown component-type".to_string(),
            ))
        });

        validation.pop_context();

        (wasm_components, wasm_builds, wasm_rpc_stub_builds)
    }

    fn convert_components<F, C>(
        source: &Path,
        validation: &mut ValidationBuilder,
        components_by_type: &mut BTreeMap<&'static str, Vec<oam::Component>>,
        component_type: &str,
        convert: F,
    ) -> Vec<C>
    where
        F: Fn(&Path, &mut ValidationBuilder, oam::Component) -> Option<C>,
    {
        components_by_type
            .remove(component_type)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|component| {
                validation.push_context("component name", component.name.clone());
                validation.push_context("component type", component.component_type.clone());
                let result = convert(source, validation, component);
                validation.pop_context();
                validation.pop_context();
                result
            })
            .collect()
    }

    fn convert_wasm_component(
        source: &Path,
        validation: &mut ValidationBuilder,
        mut component: oam::Component,
    ) -> Option<WasmComponent> {
        let properties = component.typed_properties::<raw::WasmComponentProperties>();

        if let Some(err) = properties.as_ref().err() {
            validation.add_error(format!("Failed to get component properties: {}", err))
        }

        let wasm_rpc_traits = component
            .extract_traits_by_type(&BTreeSet::from([raw::OAM_TRAIT_TYPE_WASM_RPC]))
            .remove(raw::OAM_TRAIT_TYPE_WASM_RPC)
            .unwrap_or_default();

        let mut wasm_rpc_dependencies = Vec::<ComponentName>::new();
        for wasm_rpc in wasm_rpc_traits {
            validation.push_context("trait type", wasm_rpc.trait_type.clone());

            match raw::WasmRpcTraitProperties::from_generic_trait(wasm_rpc) {
                Ok(wasm_rpc) => {
                    wasm_rpc.add_unknown_property_warns(
                        || vec![("dep component name", wasm_rpc.component_name.clone())],
                        validation,
                    );
                    wasm_rpc_dependencies.push(wasm_rpc.component_name.into())
                }
                Err(err) => validation
                    .add_error(format!("Failed to get wasm-rpc trait properties: {}", err)),
            }

            validation.pop_context();
        }

        let non_unique_wasm_rpc_dependencies = wasm_rpc_dependencies
            .iter()
            .counts()
            .into_iter()
            .filter(|(_, count)| *count > 1);
        validation.add_warns(
            non_unique_wasm_rpc_dependencies,
            |(dep_component_name, count)| {
                Some((
                    vec![],
                    format!(
                        "WASM RPC dependency specified multiple times for component: {}, count: {}",
                        dep_component_name, count
                    ),
                ))
            },
        );

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm component, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        let wasm_rpc_dependencies = wasm_rpc_dependencies
            .into_iter()
            .unique()
            .sorted()
            .collect::<Vec<_>>();

        match (properties, validation.has_any_errors()) {
            (Ok(properties), false) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                for build_step in &properties.build {
                    let has_inputs = !build_step.inputs.is_empty();
                    let has_outputs = !build_step.outputs.is_empty();

                    if (has_inputs && !has_outputs) || (!has_inputs && has_outputs) {
                        validation.push_context("command", build_step.command.clone());
                        validation.add_warn(
                            "Using inputs and outputs only has effect when both defined"
                                .to_string(),
                        );
                        validation.pop_context();
                    }
                }

                if !properties.build_profiles.is_empty() && !properties.build.is_empty() {
                    validation.add_warn(
                        "If buildProfiles is defined then build will be ignored".to_string(),
                    );
                }

                if !properties.build_profiles.is_empty()
                    && properties.default_build_profile.is_none()
                {
                    validation.add_error("If buildProfiles is defined then defaultBuildProfile also have to be defined".to_string());
                }

                if properties.build_profiles.is_empty()
                    && properties.default_build_profile.is_some()
                {
                    validation.add_error("If defaultBuildProfile is defined then buildProfiles also have to be defined".to_string());
                }

                if let Some(default_build_profile) = &properties.default_build_profile {
                    if !properties
                        .build_profiles
                        .contains_key(default_build_profile)
                    {
                        validation.add_error(format!(
                            "The defined defaultBuildProfile ({}) if not found in buildProfiles",
                            default_build_profile.log_color_error_highlight()
                        ))
                    }
                }

                match properties.component_template {
                    Some(build_template) => {
                        for (property_defined, property_name) in [
                            (properties.input_wit.is_some(), "inputWit"),
                            (properties.output_wit.is_some(), "outputWit"),
                            (!properties.build.is_empty(), "build"),
                            (!properties.build_profiles.is_empty(), "buildProfiles"),
                            (
                                properties.default_build_profile.is_some(),
                                "defaultBuildProfile",
                            ),
                            (properties.input_wasm.is_some(), "inputWasm"),
                            (properties.output_wasm.is_some(), "outputWasm"),
                        ] {
                            if property_defined {
                                validation.add_warn(format!(
                                    "Component property {} is ignored when componentTemplate is defined",
                                    property_name.log_color_error_highlight()
                                ))
                            }
                        }

                        Some(WasmComponent {
                            name: component.name.into(),
                            source: source.to_path_buf(),
                            properties: WasmComponentPropertySource::Template {
                                template_name: build_template,
                            },
                            wasm_rpc_dependencies,
                        })
                    }
                    None => {
                        for (property, property_name) in [
                            (&properties.input_wit, "inputWit"),
                            (&properties.output_wit, "outputWit"),
                            (&properties.input_wasm, "inputWasm"),
                            (&properties.output_wasm, "outputWasm"),
                        ] {
                            if property.is_none() {
                                validation.add_error(format!("Component property {} must be defined, unless componentTemplate is defined", property_name.log_color_error_highlight()))
                            }
                        }

                        Some(WasmComponent {
                            name: component.name.into(),
                            source: source.to_path_buf(),
                            properties: WasmComponentPropertySource::Concrete {
                                build: WasmComponentProperties {
                                    build: properties.build,
                                    build_profiles: properties.build_profiles,
                                    default_build_profile: properties.default_build_profile,
                                    input_wit: properties.input_wit.unwrap_or_default().into(),
                                    output_wit: properties.output_wit.unwrap_or_default().into(),
                                    input_wasm: properties.input_wasm.unwrap_or_default().into(),
                                    output_wasm: properties.output_wasm.unwrap_or_default().into(),
                                },
                            },
                            wasm_rpc_dependencies,
                        })
                    }
                }
            }
            _ => None,
        }
    }

    fn convert_wasm_build(
        source: &Path,
        validation: &mut ValidationBuilder,
        component: oam::Component,
    ) -> Option<WasmBuild> {
        let result = match component.typed_properties::<raw::ComponentBuildProperties>() {
            Ok(properties) => {
                // TODO: validate component templates
                properties.add_unknown_property_warns(Vec::new, validation);

                let wasm_rpc_stub_build = WasmBuild {
                    source: source.to_path_buf(),
                    name: component.name,
                    build_dir: properties.build_dir.map(|s| s.into()),
                    component_templates: properties.component_templates,
                    wit_deps: properties.wit_deps.into_iter().map(|s| s.into()).collect(),
                };

                Some(wasm_rpc_stub_build)
            }
            Err(err) => {
                validation.add_error(format!("Failed to get wasm build properties: {}", err));
                None
            }
        };

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm build, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        result
    }

    fn convert_wasm_rpc_stub_build(
        source: &Path,
        validation: &mut ValidationBuilder,
        component: oam::Component,
    ) -> Option<WasmRpcStubBuild> {
        let result = match component.typed_properties::<raw::ComponentStubBuildProperties>() {
            Ok(properties) => {
                properties.add_unknown_property_warns(Vec::new, validation);

                let wasm_rpc_stub_build = WasmRpcStubBuild {
                    source: source.to_path_buf(),
                    name: component.name,
                    component_name: properties.component_name.map(Into::into),
                    build_dir: properties.build_dir.map(|s| s.into()),
                    wasm: properties.wasm.map(|s| s.into()),
                    wit: properties.wit.map(|s| s.into()),
                    world: properties.world,
                    crate_version: properties.crate_version,
                    wasm_rpc_path: properties.wasm_rpc_path,
                    wasm_rpc_version: properties.wasm_rpc_version,
                };

                if wasm_rpc_stub_build.build_dir.is_some() && wasm_rpc_stub_build.wasm.is_some() {
                    validation.add_warn(
                        "Both buildDir and wasm fields are defined, wasm takes precedence"
                            .to_string(),
                    );
                }

                if wasm_rpc_stub_build.build_dir.is_some() && wasm_rpc_stub_build.wit.is_some() {
                    validation.add_warn(
                        "Both buildDir and wit fields are defined, wit takes precedence"
                            .to_string(),
                    );
                }

                if wasm_rpc_stub_build.component_name.is_some()
                    && wasm_rpc_stub_build.wasm.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) wasm rpc stub build the wasm field has no effect".to_string(),
                    );
                }

                if wasm_rpc_stub_build.component_name.is_some() && wasm_rpc_stub_build.wit.is_some()
                {
                    validation.add_warn(
                        "In common (without component name) wasm rpc stub build the wit field has no effect".to_string(),
                    );
                }

                Some(wasm_rpc_stub_build)
            }
            Err(err) => {
                validation.add_error(format!(
                    "Failed to get wasm rpc stub build properties: {}",
                    err
                ));
                None
            }
        };

        validation.add_warns(component.traits, |component_trait| {
            Some((
                vec![],
                format!(
                    "Unknown trait for wasm rpc stub build, trait type: {}",
                    component_trait.trait_type
                ),
            ))
        });

        result
    }

    pub fn validate_components(
        validation: &mut ValidationBuilder,
        components: Vec<WasmComponent>,
    ) -> BTreeMap<ComponentName, WasmComponent> {
        let (wasm_components_by_name, sources) = {
            let mut wasm_components_by_name = BTreeMap::<ComponentName, WasmComponent>::new();
            let mut sources = BTreeMap::<ComponentName, Vec<String>>::new();
            for component in components {
                sources
                    .entry(component.name.clone())
                    .and_modify(|sources| sources.push(component.source_as_string()))
                    .or_insert_with(|| vec![component.source_as_string()]);
                wasm_components_by_name.insert(component.name.clone(), component);
            }
            (wasm_components_by_name, sources)
        };

        let non_unique_components = sources.into_iter().filter(|(_, sources)| sources.len() > 1);
        validation.add_errors(non_unique_components, |(component_name, sources)| {
            Some((
                vec![("component name", component_name.0)],
                format!(
                    "Component is specified multiple times in sources: {}",
                    sources.iter().map(|s| s.log_color_highlight()).join(", ")
                ),
            ))
        });

        for (component_name, component) in &wasm_components_by_name {
            validation.push_context("source", component.source_as_string());

            validation.add_errors(&component.wasm_rpc_dependencies, |dep_component_name| {
                (!wasm_components_by_name.contains_key(dep_component_name)).then(|| {
                    (
                        vec![],
                        format!(
                            "Component {} references unknown component {} as dependency",
                            component_name.log_color_highlight(),
                            dep_component_name.log_color_error_highlight(),
                        ),
                    )
                })
            });

            validation.pop_context();
        }

        wasm_components_by_name
    }

    fn validate_wasm_builds(
        validation: &mut ValidationBuilder,
        wasm_builds: Vec<WasmBuild>,
    ) -> Option<WasmBuild> {
        if wasm_builds.len() > 1 {
            validation.add_error(format!(
                "Component Build is specified multiple times in sources: {}",
                wasm_builds
                    .iter()
                    .map(|c| format!(
                        "{} in {}",
                        c.name.log_color_highlight(),
                        c.source.log_color_highlight()
                    ))
                    .join(", ")
            ));
        }

        if wasm_builds.len() == 1 {
            for (_template_name, _build) in &wasm_builds[0].component_templates {
                // TODO: validate templates
            }
        }

        wasm_builds.into_iter().next()
    }

    fn validate_wasm_rpc_stub_builds(
        validation: &mut ValidationBuilder,
        wasm_components_by_name: &BTreeMap<ComponentName, WasmComponent>,
        wasm_rpc_stub_builds: Vec<WasmRpcStubBuild>,
    ) -> (
        Option<WasmRpcStubBuild>,
        BTreeMap<ComponentName, WasmRpcStubBuild>,
    ) {
        let (
            common_wasm_rpc_stub_builds,
            wasm_rpc_stub_builds_by_component_name,
            common_sources,
            sources,
        ) = {
            let mut common_wasm_rpc_stub_builds = Vec::<WasmRpcStubBuild>::new();
            let mut wasm_rpc_stub_builds_by_component_name =
                BTreeMap::<ComponentName, WasmRpcStubBuild>::new();

            let mut common_sources = Vec::<String>::new();
            let mut by_name_sources = BTreeMap::<ComponentName, Vec<String>>::new();

            for wasm_rpc_stub_build in wasm_rpc_stub_builds {
                match &wasm_rpc_stub_build.component_name {
                    Some(component_name) => {
                        by_name_sources
                            .entry(component_name.clone())
                            .and_modify(|sources| {
                                sources.push(wasm_rpc_stub_build.source_as_string())
                            })
                            .or_insert_with(|| vec![wasm_rpc_stub_build.source_as_string()]);
                        wasm_rpc_stub_builds_by_component_name
                            .insert(component_name.clone(), wasm_rpc_stub_build);
                    }
                    None => {
                        common_sources.push(wasm_rpc_stub_build.source_as_string());
                        common_wasm_rpc_stub_builds.push(wasm_rpc_stub_build)
                    }
                }
            }

            (
                common_wasm_rpc_stub_builds,
                wasm_rpc_stub_builds_by_component_name,
                common_sources,
                by_name_sources,
            )
        };

        let non_unique_wasm_rpc_stub_builds =
            sources.into_iter().filter(|(_, sources)| sources.len() > 1);

        validation.add_errors(
            non_unique_wasm_rpc_stub_builds,
            |(component_name, sources)| {
                Some((
                    vec![("component name", component_name.0)],
                    format!(
                        "Wasm rpc stub build is specified multiple times in sources: {}",
                        sources.iter().map(|s| s.log_color_highlight()).join(", ")
                    ),
                ))
            },
        );

        if common_sources.len() > 1 {
            validation.add_error(
                format!(
                    "Common (without component name) wasm rpc build is specified multiple times in sources: {}",
                    common_sources.iter().map(|s| s.log_color_highlight()).join(", "),
                )
            )
        }

        validation.add_errors(
            &wasm_rpc_stub_builds_by_component_name,
            |(component_name, wasm_rpc_stub_build)| {
                (!wasm_components_by_name.contains_key(component_name)).then(|| {
                    (
                        vec![("source", wasm_rpc_stub_build.source_as_string())],
                        format!(
                            "Wasm rpc stub build {} references unknown component {}",
                            wasm_rpc_stub_build.name.log_color_highlight(),
                            component_name.log_color_error_highlight(),
                        ),
                    )
                })
            },
        );

        (
            common_wasm_rpc_stub_builds.into_iter().next(),
            wasm_rpc_stub_builds_by_component_name,
        )
    }

    pub fn components(&self) -> impl Iterator<Item = (&ComponentName, &WasmComponent)> {
        self.wasm_components_by_name.iter()
    }

    pub fn component_names(&self) -> impl Iterator<Item = &ComponentName> {
        self.wasm_components_by_name.keys()
    }

    pub fn wit_deps(&self) -> Vec<PathBuf> {
        self.common_wasm_build
            .as_ref()
            .map(|wasm_build| wasm_build.wit_deps.clone())
            .unwrap_or_default()
    }

    pub fn all_wasm_rpc_dependencies(&self) -> BTreeSet<ComponentName> {
        self.wasm_components_by_name
            .iter()
            .flat_map(|(_, component)| component.wasm_rpc_dependencies.iter().cloned())
            .collect()
    }

    pub fn all_profiles(&self) -> BTreeSet<String> {
        self.component_names()
            .flat_map(|component_name| {
                self.component_properties(component_name)
                    .build_profiles
                    .keys()
                    .cloned()
            })
            .collect()
    }

    pub fn build_dir(&self) -> PathBuf {
        self.common_wasm_build
            .as_ref()
            .and_then(|build| build.build_dir.clone())
            .unwrap_or_else(|| PathBuf::from("build"))
    }

    pub fn component(&self, component_name: &ComponentName) -> &WasmComponent {
        self.wasm_components_by_name
            .get(component_name)
            .unwrap_or_else(|| panic!("Component not found: {}", component_name))
    }

    pub fn component_properties(&self, component_name: &ComponentName) -> &WasmComponentProperties {
        match &self.component(component_name).properties {
            WasmComponentPropertySource::Concrete { build } => build,
            WasmComponentPropertySource::Template { .. } => self
                .wasm_component_rendered_templates_by_name
                .get(component_name)
                .expect("Missing rendered template"),
        }
    }

    pub fn component_input_wit(&self, component_name: &ComponentName) -> PathBuf {
        let component = self.component(component_name);
        component
            .source_dir()
            .join(self.component_properties(component_name).input_wit.clone())
    }

    pub fn component_base_output_wit(&self, component_name: &ComponentName) -> PathBuf {
        self.build_dir()
            .join("base_output_wit")
            .join(component_name.as_str())
    }

    pub fn component_base_output_wit_interface_package_dir(
        &self,
        component_name: &ComponentName,
        interface_package_name: &PackageName,
    ) -> PathBuf {
        self.component_base_output_wit(component_name)
            .join(naming::wit::DEPS_DIR)
            .join(package_dep_dir_name_from_parser(interface_package_name))
            .join(naming::wit::INTERFACE_WIT_FILE_NAME)
    }

    pub fn component_output_wit(&self, component_name: &ComponentName) -> PathBuf {
        let component = self.component(component_name);
        component
            .source_dir()
            .join(self.component_properties(component_name).output_wit.clone())
    }

    pub fn component_input_wasm(&self, component_name: &ComponentName) -> PathBuf {
        let component = self.component(component_name);
        component
            .source_dir()
            .join(self.component_properties(component_name).input_wasm.clone())
    }

    pub fn component_output_wasm(&self, component_name: &ComponentName) -> PathBuf {
        let component = self.component(component_name);
        component.source_dir().join(
            self.component_properties(component_name)
                .output_wasm
                .clone(),
        )
    }

    pub fn component_build_steps<'a>(
        &'a self,
        component_name: &ComponentName,
        profile: Option<&'a str>,
    ) -> BuildStepsLookupResult<'a> {
        let component_build = self.component_properties(component_name);
        match profile {
            Some(profile) => match component_build.build_profiles.get(profile) {
                Some(build_steps) => BuildStepsLookupResult::BuildStepsForRequestedProfile {
                    profile,
                    build_steps,
                },
                None => BuildStepsLookupResult::NoBuildStepsForRequestedProfile,
            },
            None => {
                if !component_build.build_profiles.is_empty() {
                    let default_profile = component_build
                        .default_build_profile
                        .as_ref()
                        .expect("Missing build profile");

                    BuildStepsLookupResult::BuildStepsForDefaultProfile {
                        profile: default_profile,
                        build_steps: component_build
                            .build_profiles
                            .get(default_profile)
                            .expect("Missing build steps for profile"),
                    }
                } else if !component_build.build.is_empty() {
                    BuildStepsLookupResult::BuildSteps {
                        build_steps: &component_build.build,
                    }
                } else {
                    BuildStepsLookupResult::NoBuildSteps
                }
            }
        }
    }

    pub fn stub_world(&self, component_name: &ComponentName) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.world.clone())
            .flatten()
    }

    pub fn stub_crate_version(&self, component_name: &ComponentName) -> String {
        self.stub_gen_property(component_name, |build| build.crate_version.clone())
            .flatten()
            .unwrap_or_else(|| WASM_RPC_VERSION.to_string())
    }

    pub fn stub_wasm_rpc_path(&self, component_name: &ComponentName) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.wasm_rpc_path.clone())
            .flatten()
    }

    pub fn stub_wasm_rpc_version(&self, component_name: &ComponentName) -> Option<String> {
        self.stub_gen_property(component_name, |build| build.wasm_rpc_version.clone())
            .flatten()
    }

    pub fn stub_build_dir(&self, component_name: &ComponentName) -> PathBuf {
        self.stub_gen_property(component_name, |build| {
            build
                .build_dir
                .as_ref()
                .map(|build_dir| build.source_dir().join(build_dir))
        })
        .flatten()
        .unwrap_or_else(|| self.build_dir())
        .join("stub")
    }

    pub fn stub_temp_build_dir(&self, component_name: &ComponentName) -> PathBuf {
        self.stub_build_dir(component_name)
            .join(component_name.as_str())
            .join("temp-build")
    }

    pub fn stub_wasm(&self, component_name: &ComponentName) -> PathBuf {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .and_then(|build| {
                build
                    .wasm
                    .as_ref()
                    .map(|wasm| build.source_dir().join(wasm))
            })
            .unwrap_or_else(|| {
                self.stub_build_dir(component_name)
                    .join(component_name.as_str())
                    .join("stub.wasm")
            })
    }

    pub fn stub_wit(&self, component_name: &ComponentName) -> PathBuf {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .and_then(|build| build.wit.as_ref().map(|wit| build.source_dir().join(wit)))
            .unwrap_or_else(|| {
                self.stub_build_dir(component_name)
                    .join(component_name.as_str())
                    .join(naming::wit::WIT_DIR)
            })
    }

    fn stub_gen_property<T, F>(&self, component_name: &ComponentName, get_property: F) -> Option<T>
    where
        F: Fn(&WasmRpcStubBuild) -> T,
    {
        self.wasm_rpc_stub_builds_by_name
            .get(component_name)
            .map(&get_property)
            .or_else(|| self.common_wasm_rpc_stub_build.as_ref().map(get_property))
    }
}

pub type BuildStep = raw::BuildStep;

#[derive(Clone, Debug)]
pub struct WasmComponentProperties {
    pub build: Vec<raw::BuildStep>,
    pub build_profiles: HashMap<String, Vec<raw::BuildStep>>,
    pub default_build_profile: Option<String>,
    pub input_wit: PathBuf,
    pub output_wit: PathBuf,
    pub input_wasm: PathBuf,
    pub output_wasm: PathBuf,
}

#[derive(Clone, Debug)]
pub enum WasmComponentPropertySource {
    Concrete { build: WasmComponentProperties },
    Template { template_name: String },
}

#[derive(Clone, Debug)]
pub struct WasmComponent {
    pub name: ComponentName,
    pub source: PathBuf,
    pub properties: WasmComponentPropertySource,
    pub wasm_rpc_dependencies: Vec<ComponentName>,
}

impl WasmComponent {
    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }

    pub fn source_dir(&self) -> &Path {
        self.source.parent().unwrap_or_else(|| {
            panic!(
                "Failed to get parent for component {}, source: {}",
                self.name,
                self.source.display()
            )
        })
    }
}

pub enum BuildStepsLookupResult<'a> {
    NoBuildSteps,
    NoBuildStepsForRequestedProfile,
    BuildSteps {
        build_steps: &'a [BuildStep],
    },
    BuildStepsForDefaultProfile {
        profile: &'a str,
        build_steps: &'a [BuildStep],
    },
    BuildStepsForRequestedProfile {
        profile: &'a str,
        build_steps: &'a [BuildStep],
    },
}

#[derive(Clone, Debug)]
pub struct WasmBuild {
    source: PathBuf,
    name: String,
    build_dir: Option<PathBuf>,
    component_templates: HashMap<String, raw::WasmComponentTemplateProperties>,
    wit_deps: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct WasmRpcStubBuild {
    source: PathBuf,
    name: String,
    component_name: Option<ComponentName>,
    build_dir: Option<PathBuf>,
    wasm: Option<PathBuf>,
    wit: Option<PathBuf>,
    world: Option<String>,
    crate_version: Option<String>,
    wasm_rpc_path: Option<String>,
    wasm_rpc_version: Option<String>,
}

impl WasmRpcStubBuild {
    pub fn source_as_string(&self) -> String {
        self.source.to_string_lossy().to_string()
    }

    pub fn source_dir(&self) -> &Path {
        self.source
            .parent()
            .expect("Failed to get parent for source")
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::model::oam;
    use assert2::assert;

    #[test]
    fn load_app_with_warns() {
        let oam_app: oam::ApplicationWithSource = oam_app_one();
        let (app, warns, errors) = Application::from_oam_apps(vec![oam_app]).into_product();

        assert!(app.is_some());
        let app = app.unwrap();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(app.wasm_components_by_name.len() == 3);
        assert!(warns.len() == 3);
        assert!(errors.len() == 0);

        let (component_name, component) = app.wasm_components_by_name.iter().next().unwrap();

        assert!(component_name.as_str() == "component-one");
        assert!(component.name.as_str() == "component-one");
        // TODO:
        /*
        assert!(component.input_wit.to_string_lossy() == "input_wit");
        assert!(component.output_wit.to_string_lossy() == "output_wit");
        assert!(component.input_wasm.to_string_lossy() == "out/in.wasm");
        assert!(component.output_wasm.to_string_lossy() == "out/out.wasm");
        */
        assert!(component.wasm_rpc_dependencies.len() == 2);

        assert!(component.wasm_rpc_dependencies[0].as_str() == "component-three");
        assert!(component.wasm_rpc_dependencies[1].as_str() == "component-two");
    }

    #[test]
    fn load_app_with_warns_and_errors() {
        let oam_app: oam::ApplicationWithSource = oam_app_two();
        let (_app, warns, errors) = Application::from_oam_apps(vec![oam_app]).into_product();

        println!("Warns:\n{}", warns.join("\n"));
        println!("Errors:\n{}", errors.join("\n"));

        assert!(errors.len() == 2);

        assert!(errors[0].contains("component-one"));
        assert!(errors[0].contains("component-three"));
        assert!(errors[0].contains("test-oam-app-two.yaml"));

        assert!(errors[1].contains("component-one"));
        assert!(errors[1].contains("component-two"));
        assert!(errors[1].contains("test-oam-app-two.yaml"));
    }

    fn oam_app_one() -> oam::ApplicationWithSource {
        oam::ApplicationWithSource::from_yaml_string(
            "test-oam-app-one.yaml".into(),
            r#"
apiVersion: core.oam.dev/v1beta1
metadata:
  name: "App name"
kind: Application
spec:
  components:
    - name: component-one
      type: wasm
      properties:
        inputWit: input_wit
        outputWit: output_wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
        testUnknownProp: test
      traits:
        - type: wasm-rpc
          properties:
            componentName: component-two
        - type: wasm-rpc
          properties:
            componentName: component-three
            testUnknownProp: test
        - type: unknown-trait
          properties:
            testUnknownProp: test
    - name: component-two
      type: wasm
      properties:
        inputWit: input_wit
        outputWit: output_wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
    - name: component-three
      type: wasm
      properties:
        inputWit: input_wit
        outputWit: output_wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
"#
            .to_string(),
        )
        .unwrap()
    }

    fn oam_app_two() -> oam::ApplicationWithSource {
        oam::ApplicationWithSource::from_yaml_string(
            "test-oam-app-two.yaml".into(),
            r#"
apiVersion: core.oam.dev/v1beta1
metadata:
  name: "App name"
kind: Application
spec:
  components:
    - name: component-one
      type: wasm
      properties:
        inputWit: input-wit
        outputWit: input-wit
        inputWasm: out/in.wasm
        outputWasm: out/out.wasm
        testUnknownProp: test
      traits:
        - type: wasm-rpc
          properties:
            componentName: component-two
        - type: wasm-rpc
          properties:
            componentName: component-three
            testUnknownProp: test
        - type: unknown-trait
          properties:
            testUnknownProp: test
    - name: component-one
      type: unknown-component-type
      properties:
"#
            .to_string(),
        )
        .unwrap()
    }
}
