// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::config::{AuthSecret, AuthenticationConfig, Profile, ProfileConfig, ProfileName};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::{log_action, log_warn_action, logln, LogColorize};
use crate::model::app::{
    AppComponentName, BinaryComponentSource, DependencyType, HttpApiDeploymentSite,
};
use crate::model::component::AppComponentType;
use crate::model::text::fmt::{log_error, log_warn};
use crate::model::{ComponentName, Format, NewInteractiveApp, WorkerName};
use anyhow::bail;
use colored::Colorize;
use golem_client::model::Account;
use golem_client::model::HttpApiDefinitionRequest;
use golem_common::model::ComponentVersion;
use golem_templates::model::{ComposableAppGroupName, GuestLanguage, PackageName};
use inquire::error::InquireResult;
use inquire::validator::{ErrorMessage, Validation};
use inquire::{Confirm, CustomType, InquireError, Select, Text};
use itertools::Itertools;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;
use uuid::Uuid;

// NOTE: in the interactive handler is it okay to _read_ context (e.g. read selected component names)
//       but mutations of state or the app should be done in other handlers
pub struct InteractiveHandler {
    ctx: Arc<Context>,
}

impl InteractiveHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    // NOTE: static because local server hook has limited access to state
    pub fn confirm_auto_start_local_server(yes: bool) -> anyhow::Result<bool> {
        confirm(
            yes,
            true,
            "Do you want to use the local server for the current session?",
            Some("Tip: you can also use the 'golem server run' command in another terminal to keep the local server running for local development!"),
        )
    }

    // NOTE: static because happens during context construction
    pub fn confirm_manifest_profile_warning(yes: bool) -> anyhow::Result<bool> {
        confirm(
            yes,
            true,
            "Profiles defined in the application manifest were loaded with warnings.\nDo you want to continue?",
            None,
        )
    }

    // NOTE: static because happens inside app context lock
    pub fn confirm_manifest_app_warning(yes: bool) -> anyhow::Result<bool> {
        confirm(
            yes,
            true,
            "Application manifest was loaded with warnings.\nDo you want to continue?",
            None,
        )
    }

    pub fn confirm_auto_deploy_component(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Component {} was not found between deployed components, do you want to deploy it, then continue?",
                component_name.0.log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_redeploy_workers(&self, number_of_workers: usize) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Redeploying will {} then recreate {} worker(s), do you want to continue?",
                "delete".log_color_warn(),
                number_of_workers.to_string().log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_undeploy_api_from_sites_for_redeploy(
        &self,
        api: &str,
        sites: &[(HttpApiDeploymentSite, String)],
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Redeploying will temporarily undeploy HTTP API {} from the following site(s):\n{}\nDo you want to continue?",
                api.log_color_highlight(),
                sites
                    .iter()
                    .map(|target| format!("- {}, used API version: {}", target.0.to_string().log_color_highlight(), target.1.log_color_highlight()))
                    .join("\n"),
            ),
            None,
        )
    }

    pub fn confirm_update_to_latest(
        &self,
        component_name: &ComponentName,
        worker_name: &WorkerName,
        target_version: ComponentVersion,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!("Worker {}/{} will be updated to the latest component version: {}. Do you want to continue?",
                    component_name.0.log_color_highlight(),
                    worker_name.0.log_color_highlight(),
                    target_version.to_string().log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_delete_account(&self, account: &Account) -> anyhow::Result<bool> {
        self.confirm(
            false,
            format!(
                "Are you sure you want to delete the requested account? ({}, {})",
                account.name.log_color_highlight(),
                account.email.log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_plugin_installation_changes(
        &self,
        component: &AppComponentName,
        rendered_steps: &[String],
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!("The following changes will be applied to the installed plugins of component {}:\n{}\nDo you want to continue?",
                    component.to_string().log_color_highlight(),
                    rendered_steps.iter().map(|s| format!(" - {s}")).collect::<Vec<_>>().join("\n")
            ),
            None,
        )
    }

    pub fn confirm_deployment_installation_changes(
        &self,
        site: &str,
        rendered_steps: &[String],
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "The following changes will be applied to site {}:\n{}\nDo you want to continue?",
                site.to_string().log_color_highlight(),
                rendered_steps
                    .iter()
                    .map(|s| format!(" - {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            None,
        )
    }

    pub fn create_profile(&self) -> anyhow::Result<(ProfileName, Profile, bool)> {
        if !self.confirm(
            true,
            concat!(
                "Do you want to create a new profile interactively?\n",
                "If not, please specify the profile name as a command argument."
            ),
            None,
        )? {
            bail!(NonSuccessfulExit);
        }

        let profile_name = Text::new("Profile Name: ")
            .with_validator(|value: &str| {
                if ProfileName::from(value).is_builtin() {
                    return Ok(Validation::Invalid(ErrorMessage::from(
                        "The requested profile name is a builtin one, please choose another name!",
                    )));
                }
                Ok(Validation::Valid)
            })
            .prompt()?;

        let component_service_url = CustomType::<Url>::new("Component service URL:").prompt()?;

        let worker_service_url = CustomType::<OptionalUrl>::new(
            "Worker service URL (empty to use component service url):",
        )
        .prompt()?
        .0;

        let cloud_service_url = CustomType::<OptionalUrl>::new(
            "Cloud service URL (empty to use component service url)",
        )
        .prompt()?
        .0;

        let default_format =
            Select::new("Default output format:", Format::iter().collect::<Vec<_>>())
                .with_starting_cursor(2)
                .prompt()?;

        let static_token = CustomType::<OptionalAuthSecret>::new(
            "Static token for authentication (empty to use interactive authentication via OAuth2):",
        )
        .prompt()?
        .0;

        let auth = if let Some(static_token) = static_token {
            AuthenticationConfig::static_token(static_token.0)
        } else {
            AuthenticationConfig::empty_oauth2()
        };

        let profile = Profile {
            custom_url: Some(component_service_url),
            custom_cloud_url: cloud_service_url,
            custom_worker_url: worker_service_url,
            allow_insecure: false,
            config: ProfileConfig { default_format },
            auth,
        };

        let set_as_active = Confirm::new("Set as active profile?")
            .with_default(false)
            .prompt()?;

        Ok((profile_name.into(), profile, set_as_active))
    }

    pub fn select_component_for_repl(
        &self,
        component_names: Vec<ComponentName>,
    ) -> anyhow::Result<ComponentName> {
        Ok(Select::new(
            "Select a component to be used in Rib REPL:",
            component_names,
        )
        .prompt()?)
    }

    pub async fn create_component_dependency(
        &self,
        component_name: Option<AppComponentName>,
        target_component_name: Option<AppComponentName>,
        target_component_file: Option<PathBuf>,
        target_component_url: Option<Url>,
        dependency_type: Option<DependencyType>,
    ) -> anyhow::Result<Option<(AppComponentName, BinaryComponentSource, DependencyType)>> {
        let component_type_by_name =
            async |component_name: &AppComponentName| -> anyhow::Result<AppComponentType> {
                let app_ctx = self.ctx.app_context_lock().await;
                let app_ctx = app_ctx.some_or_err()?;
                Ok(app_ctx
                    .application
                    .component_properties(component_name, self.ctx.build_profile())
                    .component_type())
            };

        fn validate_component_type_for_dependency_type(
            dependency_type: DependencyType,
            component_type: AppComponentType,
        ) -> bool {
            match dependency_type {
                DependencyType::DynamicWasmRpc | DependencyType::StaticWasmRpc => {
                    match component_type {
                        AppComponentType::Durable | AppComponentType::Ephemeral => true,
                        AppComponentType::Library => false,
                    }
                }
                DependencyType::Wasm => match component_type {
                    AppComponentType::Durable | AppComponentType::Ephemeral => false,
                    AppComponentType::Library => true,
                },
            }
        }

        let component_names = {
            let app_ctx = self.ctx.app_context_lock().await;
            let app_ctx = app_ctx.some_or_err()?;
            app_ctx
                .application
                .component_names()
                .cloned()
                .collect::<Vec<_>>()
        };

        let component_name = {
            match component_name {
                Some(component_name) => {
                    if !component_names.contains(&component_name) {
                        log_error(format!(
                            "Component {} not found, available components: {}",
                            component_name.as_str().log_color_highlight(),
                            component_names
                                .iter()
                                .map(|name| name.as_str().log_color_highlight())
                                .join(", ")
                        ));
                        bail!(NonSuccessfulExit);
                    }
                    component_name
                }
                None => {
                    if component_names.is_empty() {
                        log_error(format!(
                            "No components found! Use the '{}' subcommand to create components.",
                            "component new".log_color_highlight()
                        ));
                        bail!(NonSuccessfulExit);
                    }

                    match Select::new(
                        "Select a component to which you want to add a new dependency:",
                        component_names.clone(),
                    )
                    .prompt()
                    .none_if_not_interactive_logged()?
                    {
                        Some(component_name) => component_name,
                        None => return Ok(None),
                    }
                }
            }
        };

        let component_type = component_type_by_name(&component_name).await?;

        log_action(
            "Selected",
            format!(
                "component {} with component type {}",
                component_name.as_str().log_color_highlight(),
                component_type.to_string().log_color_highlight()
            ),
        );

        let dependency_type = {
            let (offered_dependency_types, valid_dependency_types) = match component_type {
                AppComponentType::Durable | AppComponentType::Ephemeral => (
                    vec![DependencyType::DynamicWasmRpc, DependencyType::Wasm],
                    vec![
                        DependencyType::DynamicWasmRpc,
                        DependencyType::StaticWasmRpc,
                        DependencyType::Wasm,
                    ],
                ),
                AppComponentType::Library => {
                    (vec![DependencyType::Wasm], vec![DependencyType::Wasm])
                }
            };

            match dependency_type {
                Some(dependency_type) => {
                    if !valid_dependency_types.contains(&dependency_type) {
                        log_error(format!(
                            "The requested {} dependency type is not valid for {} component, valid dependency types: {}",
                            dependency_type.as_str().log_color_highlight(),
                            component_name.as_str().log_color_highlight(),
                            valid_dependency_types
                                .iter()
                                .map(|name| name.as_str().log_color_highlight())
                                .join(", ")
                        ));
                        bail!(NonSuccessfulExit);
                    }
                    dependency_type
                }
                None => {
                    match Select::new("Select dependency type:", offered_dependency_types)
                        .prompt()
                        .none_if_not_interactive_logged()?
                    {
                        Some(dependency_type) => dependency_type,
                        None => return Ok(None),
                    }
                }
            }
        };

        let target_component_source = match (
            target_component_name,
            target_component_file,
            target_component_url,
        ) {
            (None, Some(target_component_file), None) => BinaryComponentSource::LocalFile {
                path: target_component_file,
            },
            (None, None, Some(target_component_url)) => BinaryComponentSource::Url {
                url: target_component_url,
            },
            (Some(target_component_name), None, None) => {
                if !component_names.contains(&target_component_name) {
                    log_error(format!(
                        "Target component {} not found, available components: {}",
                        target_component_name.as_str().log_color_highlight(),
                        component_names
                            .iter()
                            .map(|name| name.as_str().log_color_highlight())
                            .join(", ")
                    ));
                    bail!(NonSuccessfulExit);
                }

                let target_component_type = component_type_by_name(&target_component_name).await?;
                if !validate_component_type_for_dependency_type(
                    dependency_type,
                    target_component_type,
                ) {
                    log_error(
                        format!(
                            "The target component type {} is not compatible with the selected dependency type {}!",
                            target_component_type.to_string().log_color_highlight(),
                            dependency_type.as_str().log_color_highlight(),
                        )
                    );
                    logln("");
                    logln("Use a different target component or dependency type.");
                }

                BinaryComponentSource::AppComponent {
                    name: target_component_name,
                }
            }
            _ => {
                let target_component_names = {
                    let app_ctx = self.ctx.app_context_lock().await;
                    let app_ctx = app_ctx.some_or_err()?;
                    app_ctx
                        .application
                        .component_names()
                        .filter(|component_name| {
                            validate_component_type_for_dependency_type(
                                dependency_type,
                                app_ctx
                                    .application
                                    .component_properties(component_name, self.ctx.build_profile())
                                    .component_type(),
                            )
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };

                if target_component_names.is_empty() {
                    log_error(
                        "No target components are available for the selected dependency type!",
                    );
                    bail!(NonSuccessfulExit);
                }

                match Select::new(
                    "Select target dependency component:",
                    target_component_names,
                )
                .prompt()
                .none_if_not_interactive_logged()?
                {
                    Some(target_component_name) => BinaryComponentSource::AppComponent {
                        name: target_component_name,
                    },
                    None => return Ok(None),
                }
            }
        };

        Ok(Some((
            component_name,
            target_component_source,
            dependency_type,
        )))
    }

    pub fn select_new_api_definition_version(
        &self,
        api_definition: &HttpApiDefinitionRequest,
    ) -> anyhow::Result<Option<String>> {
        Text::new("Please specify a new API definition version:")
            .with_initial_value(&api_definition.version)
            .with_validator({
                let current_version = api_definition.version.clone();
                move |value: &str| {
                    if value == current_version {
                        return Ok(Validation::Invalid(ErrorMessage::Custom(
                            "Please select a new version!".to_string(),
                        )));
                    }
                    if value.contains(char::is_whitespace) {
                        return Ok(Validation::Invalid(ErrorMessage::Custom(
                            "Please specify a version without whitespaces!".to_string(),
                        )));
                    }
                    if value.is_empty() {
                        return Ok(Validation::Invalid(ErrorMessage::Custom(
                            "Please specify a non-empty version!".to_string(),
                        )));
                    }

                    Ok(Validation::Valid)
                }
            })
            .prompt()
            .none_if_not_interactive_logged()
    }

    pub fn select_new_app_name_and_components(&self) -> anyhow::Result<Option<NewInteractiveApp>> {
        let Some(app_name) = Text::new("Application name:")
            .with_validator(|value: &str| {
                if std::env::current_dir()?.join(value).exists() {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(
                        "The specified application name already exists as a directory!".to_string(),
                    )));
                }
                Ok(Validation::Valid)
            })
            .prompt()
            .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };

        let mut existing_component_names = HashSet::<String>::new();
        let mut templated_component_names = Vec::<(String, PackageName)>::new();

        loop {
            let Some(templated_component_name) = self
                .select_new_component_template_and_package_name(existing_component_names.clone())?
            else {
                return Ok(None);
            };

            let Some(choice) = Select::new(
                "Add another component or create application?",
                AddComponentOrCreateApp::iter().collect_vec(),
            )
            .prompt()
            .none_if_not_interactive_logged()?
            else {
                return Ok(None);
            };

            match choice {
                AddComponentOrCreateApp::AddComponent => {
                    existing_component_names
                        .insert(templated_component_name.1.to_string_with_colon());
                    templated_component_names.push(templated_component_name);
                    continue;
                }
                AddComponentOrCreateApp::CreateApp => {
                    templated_component_names.push(templated_component_name);
                    break;
                }
            }
        }

        Ok(Some(NewInteractiveApp {
            app_name,
            templated_component_names,
        }))
    }

    pub fn select_new_component_template_and_package_name(
        &self,
        existing_component_names: HashSet<String>,
    ) -> anyhow::Result<Option<(String, PackageName)>> {
        let Some(language) = Select::new(
            "Select language for the new component",
            GuestLanguage::iter().collect(),
        )
        .prompt()
        .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };

        let template_options = self
            .ctx
            .templates()
            .get(&language)
            .unwrap()
            .get(&ComposableAppGroupName::default())
            .unwrap()
            .components
            .iter()
            .map(|(name, template)| TemplateOption {
                template_name: name.to_string(),
                description: template.description.clone(),
            })
            .collect::<Vec<_>>();

        let Some(template) =
            Select::new("Select a template for the new component:", template_options)
                .prompt()
                .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };

        let Some(component_name) = Text::new("Component Package Name:")
            .with_validator(move |value: &str| {
                if existing_component_names.contains(value) {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(
                        "The component name already exists!".to_string(),
                    )));
                }

                if let Err(error) = PackageName::from_str(value) {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(error)));
                }

                Ok(Validation::Valid)
            })
            .prompt()
            .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };

        Ok(Some((
            format!("{}/{}", language.id(), template.template_name),
            PackageName::from_str(&component_name).unwrap(),
        )))
    }

    fn confirm<M: AsRef<str>>(
        &self,
        default: bool,
        message: M,
        extra_hint: Option<&str>,
    ) -> anyhow::Result<bool> {
        confirm(self.ctx.yes(), default, message, extra_hint)
    }
}

#[derive(Debug, Clone)]
struct OptionalUrl(Option<Url>);

impl Display for OptionalUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            None => Ok(()),
            Some(url) => write!(f, "{url}"),
        }
    }
}

impl FromStr for OptionalUrl {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            Ok(OptionalUrl(None))
        } else {
            Ok(OptionalUrl(Some(Url::from_str(s)?)))
        }
    }
}

#[derive(Clone, Copy)]
pub struct OptionalAuthSecret(Option<AuthSecret>);

impl Display for OptionalAuthSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            None => Ok(()),
            Some(value) => write!(f, "{value}"),
        }
    }
}

impl FromStr for OptionalAuthSecret {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            Ok(Self(None))
        } else {
            Ok(Self(Some(AuthSecret(Uuid::from_str(s)?))))
        }
    }
}

trait InquireResultExtensions<T> {
    fn none_if_not_interactive(self) -> anyhow::Result<Option<T>>;

    fn none_if_not_interactive_logged(self) -> anyhow::Result<Option<T>>
    where
        Self: Sized,
    {
        self.none_if_not_interactive().inspect(|value| {
            if value.is_none() {
                logln("");
                log_warn("Detected non-interactive environment, stopping interactive wizard");
                logln("");
            }
        })
    }
}

impl<T> InquireResultExtensions<T> for InquireResult<T> {
    fn none_if_not_interactive(self) -> anyhow::Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(err) if is_interactive_not_available_inquire_error(&err) => Ok(None),
            Err(err) => Err(err.into()),
        }
    }
}

fn is_interactive_not_available_inquire_error(err: &InquireError) -> bool {
    match err {
        InquireError::NotTTY => true,
        InquireError::InvalidConfiguration(_) => false,
        InquireError::IO(_) => {
            // NOTE: we consider IO errors as "not interactive" in general, e.g. currently this case
            //       triggers if stdin is not available
            true
        }
        InquireError::OperationCanceled => false,
        InquireError::OperationInterrupted => false,
        InquireError::Custom(_) => false,
    }
}

fn confirm<M: AsRef<str>>(
    yes: bool,
    default: bool,
    message: M,
    extra_hint: Option<&str>,
) -> anyhow::Result<bool> {
    const YES_FLAG_HINT: &str = "To automatically confirm such questions use the '--yes' flag.";

    logln("");

    if yes {
        log_warn_action("Auto confirming", "");
        for line in message.as_ref().cyan().lines() {
            logln(format!("> {line}"));
        }
        return Ok(true);
    }

    let hint = match extra_hint {
        Some(extra_hint) => format!("{YES_FLAG_HINT} {extra_hint}"),
        None => YES_FLAG_HINT.to_string(),
    };

    let result = match Confirm::new(message.as_ref())
        .with_help_message(&hint)
        .with_default(default)
        .prompt()
    {
        Ok(result) => Ok(result),
        Err(error) => {
            if is_interactive_not_available_inquire_error(&error) {
                log_warn(
                    "The current input device is not an interactive one,\ndefaulting to \"false\"",
                );
                Ok(false)
            } else {
                Err(error.into())
            }
        }
    };

    logln("");

    result
}

struct TemplateOption {
    pub template_name: String,
    pub description: String,
}

impl Display for TemplateOption {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}",
            self.template_name.log_color_highlight(),
            self.description
        )
    }
}

#[derive(Debug, Clone, Copy, EnumIter)]
enum AddComponentOrCreateApp {
    AddComponent,
    CreateApp,
}

impl Display for AddComponentOrCreateApp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AddComponentOrCreateApp::AddComponent => write!(f, "Add another component"),
            AddComponentOrCreateApp::CreateApp => write!(f, "Create application"),
        }
    }
}
