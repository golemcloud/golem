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
use crate::log::{log_warn, log_warn_action, logln, LogColorize};
use crate::model::format::Format;
use crate::model::worker::WorkerName;
use crate::model::{GuestLanguage, NewInteractiveApp};
use anyhow::bail;
use colored::Colorize;
use golem_client::model::Account;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use indoc::formatdoc;
use inquire::error::InquireResult;
use inquire::validator::{ErrorMessage, Validation};
use inquire::{Confirm, CustomType, InquireError, Select, Text};
use itertools::Itertools;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use url::Url;

// NOTE: in the interactive handler is it okay to _read_ context (e.g. read selected component names)
//       but mutations of state or the app should be done in other handlers
pub struct InteractiveHandler {
    ctx: Arc<Context>,
}

impl InteractiveHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
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

    pub fn confirm_deploy_by_plan(
        &self,
        application_name: &ApplicationName,
        environment_name: &EnvironmentName,
        server_formatted: &str,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            formatdoc! { "
                The above plan will be applied to the following environment:
                    Application Name: {}
                    Environment Name: {}
                    Server          : {}

                Do you want to continue the deployment?",
                application_name.0.log_color_highlight(),
                environment_name.0.log_color_highlight(),
                server_formatted.log_color_highlight(),
            },
            None,
        )
    }

    pub fn confirm_environment_deployment_options(&self) -> anyhow::Result<bool> {
        self.confirm(
            true,
            formatdoc! { "
                To continue the current command the deployments options must be updated!
                Do you want to apply the changes now?",
            },
            None,
        )
    }

    pub fn confirm_register_missing_domain(&self, domain: &Domain) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "The following domain is not registered for the environment: {}\nDo you want to register it?",
                domain.0.log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_staging_next_step(&self) -> anyhow::Result<bool> {
        self.confirm(true, "Continue with the next staging step?", None)
    }

    pub fn confirm_auto_deploy_component(
        &self,
        component_name: &ComponentName,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Component {} was not found between deployed components, do you want to deploy the application, then continue?",
                component_name.0.log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_reset_http_api_defs(&self, rendered_steps: &[String]) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Resetting HTTP API definitions will apply the following changes:\n{}\nDo you want to continue?",
                rendered_steps.iter().map(|s| format!(" - {s}")).collect::<Vec<_>>().join("\n")
            ),
            None,
        )
    }

    pub fn confirm_redeploy_agents(&self, number_of_agents: usize) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "Redeploying will {} then recreate {} agents(s), do you want to continue?",
                "delete".log_color_warn(),
                number_of_agents.to_string().log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_deleting_agents(&self, number_of_agents: usize) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!(
                "{} {} agents(s), do you want to continue?",
                "Deleting".log_color_warn(),
                number_of_agents.to_string().log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_update_to_current(
        &self,
        component_name: &ComponentName,
        worker_name: &WorkerName,
        target_revision: ComponentRevision,
    ) -> anyhow::Result<bool> {
        self.confirm(
            true,
            format!("Agent {}/{} will be updated to the current component revision: {}. Do you want to continue?",
                    component_name.0.log_color_highlight(),
                    worker_name.0.log_color_highlight(),
                    target_revision.to_string().log_color_highlight()
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
                account.email.0.log_color_highlight()
            ),
            None,
        )
    }

    pub fn confirm_plugin_installation_changes(
        &self,
        component: &ComponentName,
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

        let registry_service_url = CustomType::<Url>::new("Registry service URL:").prompt()?;

        let worker_service_url = CustomType::<OptionalUrl>::new(
            "Worker service URL (empty to use component service url):",
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
            custom_url: Some(registry_service_url),
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

    pub fn select_repl_language(
        &self,
        used_languages: &HashSet<GuestLanguage>,
    ) -> anyhow::Result<GuestLanguage> {
        Ok(Select::new(
            "Select REPL language:",
            used_languages
                .iter()
                .cloned()
                .sorted()
                .chain(
                    GuestLanguage::iter()
                        .filter(|lang| !used_languages.contains(lang))
                        .sorted(),
                )
                .collect::<Vec<_>>(),
        )
        .prompt()?)
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

    pub fn select_new_app_name_and_components(&self) -> anyhow::Result<Option<NewInteractiveApp>> {
        let Some(app_name) = Text::new("Application name:")
            .with_validator(|value: &str| {
                if std::env::current_dir()?.join(value).exists() {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(
                        "The specified application name already exists as a directory!".to_string(),
                    )));
                }

                if let Err(error) = ApplicationName::from_str(value) {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(error)));
                }

                Ok(Validation::Valid)
            })
            .prompt()
            .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };
        let app_name = ApplicationName(app_name);

        let mut existing_component_names = HashSet::<String>::new();
        let mut templated_component_names = Vec::<(String, ComponentName)>::new();

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
                    existing_component_names.insert(templated_component_name.1 .0.clone());
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
    ) -> anyhow::Result<Option<(String, ComponentName)>> {
        let available_languages = self
            .ctx
            .templates(self.ctx.dev_mode())
            .keys()
            .collect::<HashSet<_>>();

        let Some(language) = Select::new(
            "Select language for the new component",
            GuestLanguage::iter()
                .filter(|lang| available_languages.contains(lang))
                .collect(),
        )
        .prompt()
        .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };

        let template_options = self
            .ctx
            .templates(self.ctx.dev_mode())
            .get(&language)
            .unwrap()
            .get(self.ctx.template_group())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Template group not found: {}",
                    self.ctx.template_group().as_str().log_color_highlight()
                )
            })?
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

                if let Err(error) = ComponentName::from_str(value) {
                    return Ok(Validation::Invalid(ErrorMessage::Custom(error)));
                }

                Ok(Validation::Valid)
            })
            .with_placeholder("package-name:component-name")
            .prompt()
            .none_if_not_interactive_logged()?
        else {
            return Ok(None);
        };
        let component_name = ComponentName(component_name);

        Ok(Some((
            format!("{}/{}", language.id(), template.template_name),
            component_name,
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

#[derive(Clone)]
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
            Ok(Self(Some(AuthSecret(s.to_string()))))
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
                logln("\n\n");
                log_warn(
                    "The current input device is not an interactive one, defaulting to \"false\"",
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
    CreateApp,
    AddComponent,
}

impl Display for AddComponentOrCreateApp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AddComponentOrCreateApp::AddComponent => write!(f, "Add another component"),
            AddComponentOrCreateApp::CreateApp => write!(f, "Create application"),
        }
    }
}
