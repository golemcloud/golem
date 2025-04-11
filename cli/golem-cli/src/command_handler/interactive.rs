// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::config::{
    CloudProfile, OssProfile, Profile, ProfileConfig, ProfileKind, ProfileName, CLOUD_URL,
    DEFAULT_OSS_URL,
};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::{log_warn_action, LogColorize};
use crate::model::text::fmt::log_warn;
use crate::model::{ComponentName, Format};
use anyhow::{anyhow, bail};
use colored::Colorize;
use golem_cloud_client::model::Account;
use inquire::validator::{ErrorMessage, Validation};
use inquire::{Confirm, CustomType, InquireError, Select, Text};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::sync::Arc;
use strum::IntoEnumIterator;
use url::Url;

pub struct InteractiveHandler {
    ctx: Arc<Context>,
}

impl InteractiveHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
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
        )
    }

    pub fn create_profile(&self) -> anyhow::Result<(ProfileName, Profile, bool)> {
        if !self.confirm(
            true,
            concat!(
                "Do you want to create a new profile interactively?\n",
                "If not, please specify the profile name as a command argument."
            ),
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

        let profile_kind =
            Select::new("Profile kind:", ProfileKind::iter().collect::<Vec<_>>()).prompt()?;

        let component_service_url = CustomType::<Url>::new("Component service URL:")
            .with_default(match profile_kind {
                ProfileKind::Oss => Url::parse(DEFAULT_OSS_URL)?,
                ProfileKind::Cloud => Url::parse(CLOUD_URL)?,
            })
            .prompt()?;

        let worker_service_url = CustomType::<OptionalUrl>::new(
            "Worker service URL (empty to use component service url):",
        )
        .prompt()?
        .0;

        let cloud_service_url = match profile_kind {
            ProfileKind::Oss => None,
            ProfileKind::Cloud => {
                CustomType::<OptionalUrl>::new(
                    "Cloud service URL (empty to use component service url)",
                )
                .prompt()?
                .0
            }
        };

        let default_format =
            Select::new("Default output format:", Format::iter().collect::<Vec<_>>())
                .with_starting_cursor(2)
                .prompt()?;

        let profile = match profile_kind {
            ProfileKind::Oss => Profile::Golem(OssProfile {
                url: component_service_url,
                worker_url: worker_service_url,
                allow_insecure: false,
                config: ProfileConfig { default_format },
            }),
            ProfileKind::Cloud => Profile::GolemCloud(CloudProfile {
                custom_url: Some(component_service_url),
                custom_cloud_url: cloud_service_url,
                custom_worker_url: worker_service_url,
                allow_insecure: false,
                config: ProfileConfig { default_format },
                auth: None,
            }),
        };

        let set_as_active = Confirm::new("Set as active profile?")
            .with_default(false)
            .prompt()?;

        Ok((profile_name.into(), profile, set_as_active))
    }

    pub fn select_component(
        &self,
        component_names: Vec<ComponentName>,
    ) -> anyhow::Result<ComponentName> {
        Ok(Select::new(
            "Select a component to be used in Rib REPL:",
            component_names,
        )
        .prompt()?)
    }

    fn confirm<M: AsRef<str>>(&self, default: bool, message: M) -> anyhow::Result<bool> {
        const YES_FLAG_HINT: &str = "To automatically confirm such questions use the '--yes' flag.";

        if self.ctx.yes() {
            log_warn_action(
                "Auto confirming",
                format!("question: \"{}\"", message.as_ref().cyan()),
            );
            return Ok(true);
        }

        match Confirm::new(message.as_ref())
            .with_help_message(YES_FLAG_HINT)
            .with_default(default)
            .prompt()
        {
            Ok(result) => Ok(result),
            Err(error) => match error {
                InquireError::NotTTY => {
                    log_warn("The current input device is not a teletype,\ndefaulting to \"false\" as answer for the confirm question.");
                    Ok(false)
                }
                other => Err(anyhow!("{}", other)),
            },
        }
    }
}

#[derive(Debug, Clone)]
struct OptionalUrl(Option<Url>);

impl Display for OptionalUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            None => Ok(()),
            Some(url) => write!(f, "{}", url),
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
