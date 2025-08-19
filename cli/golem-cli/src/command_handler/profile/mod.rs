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

pub mod config;

use crate::command::profile::ProfileSubcommand;
use crate::command_handler::Handlers;
use crate::config::{
    AuthenticationConfig, Config, NamedProfile, Profile, ProfileConfig, ProfileName,
};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::{log_action, log_warn_action, LogColorize};
use crate::model::text::fmt::log_error;
use crate::model::{Format, ProfileView};
use anyhow::bail;
use std::collections::BTreeMap;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

pub struct ProfileCommandHandler {
    ctx: Arc<Context>,
}

impl ProfileCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, subcommand: ProfileSubcommand) -> anyhow::Result<()> {
        match subcommand {
            ProfileSubcommand::New {
                name,
                set_active,
                component_url,
                worker_url,
                cloud_url,
                default_format,
                allow_insecure,
                static_token,
            } => self.cmd_new(
                name,
                set_active,
                component_url,
                worker_url,
                cloud_url,
                default_format,
                allow_insecure,
                static_token,
            ),
            ProfileSubcommand::List => self.cmd_list(),
            ProfileSubcommand::Switch { profile_name } => self.cmd_switch(profile_name),
            ProfileSubcommand::Get { profile_name } => self.cmd_get(profile_name),
            ProfileSubcommand::Delete { profile_name } => self.cmd_delete(profile_name),
            ProfileSubcommand::Config {
                profile_name,
                subcommand,
            } => {
                self.ctx
                    .profile_config_handler()
                    .handler_subcommand(profile_name, subcommand)
                    .await
            }
        }
    }

    fn cmd_new(
        &self,
        name: Option<ProfileName>,
        set_active: bool,
        component_url: Option<Url>,
        worker_url: Option<Url>,
        cloud_url: Option<Url>,
        default_format: Format,
        allow_insecure: bool,
        static_token: Option<Uuid>,
    ) -> anyhow::Result<()> {
        let (name, profile, set_active) = match name {
            Some(name) => {
                if name.is_builtin() {
                    log_error("The requested profile name {} is a builtin profile. Please choose another profile name!");
                    bail!(NonSuccessfulExit);
                }

                let auth = if let Some(static_token) = static_token {
                    // TODO: we may want to read from prompt instead of reading parameter
                    AuthenticationConfig::static_token(static_token)
                } else {
                    AuthenticationConfig::empty_oauth2()
                };

                let profile = Profile {
                    custom_url: component_url,
                    custom_cloud_url: cloud_url,
                    custom_worker_url: worker_url,
                    allow_insecure,
                    config: ProfileConfig { default_format },
                    auth,
                };

                (name, profile, set_active)
            }
            None => self.ctx.interactive_handler().create_profile()?,
        };

        log_action(
            "Creating",
            format!("new profile: {}", name.0.log_color_highlight()),
        );
        Config::set_profile(name.clone(), profile, self.ctx.config_dir())?;

        if set_active {
            log_action(
                "Setting ",
                format!(
                    "new profile {} as the active one",
                    name.0.log_color_highlight()
                ),
            );
            Config::set_active_profile_name(name, self.ctx.config_dir())?;
        };

        Ok(())
    }

    fn cmd_list(&self) -> anyhow::Result<()> {
        let config = Config::from_dir(self.ctx.config_dir())?;
        let default_profile_name = config.default_profile_name();

        let sorted_profiles = config.profiles.into_iter().collect::<BTreeMap<_, _>>();

        let profiles = sorted_profiles
            .into_iter()
            .map(|(name, profile)| {
                ProfileView::from_profile(&default_profile_name, NamedProfile { name, profile })
            })
            .collect::<Vec<_>>();

        self.ctx.log_handler().log_view(&profiles);

        Ok(())
    }

    fn cmd_switch(&self, profile_name: ProfileName) -> anyhow::Result<()> {
        Config::set_active_profile_name(profile_name.clone(), self.ctx.config_dir())?;

        log_action(
            "Switched",
            format!("to profile: {}", profile_name.0.log_color_highlight()),
        );

        Ok(())
    }

    fn cmd_get(&self, profile_name: Option<ProfileName>) -> anyhow::Result<()> {
        let default_profile = Config::get_default_profile(self.ctx.config_dir())?;
        let default_profile_name = default_profile.name.clone();

        let profile = match profile_name {
            Some(profile_name) => {
                match Config::get_profile(self.ctx.config_dir(), &profile_name)? {
                    Some(profile) => profile,
                    None => {
                        log_error(format!(
                            "Profile {} not found",
                            profile_name.0.log_color_error_highlight()
                        ));
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            None => default_profile,
        };

        self.ctx
            .log_handler()
            .log_view(&ProfileView::from_profile(&default_profile_name, profile));

        Ok(())
    }

    fn cmd_delete(&self, profile_name: ProfileName) -> anyhow::Result<()> {
        if profile_name.is_builtin() {
            log_error(format!(
                "Cannot delete builtin profile: {}",
                profile_name.0.log_color_error_highlight()
            ));
            bail!(NonSuccessfulExit);
        }

        // Check if we're trying to delete the currently active profile
        let config = Config::from_dir(self.ctx.config_dir())?;
        let current_active_profile = config.default_profile_name();

        if profile_name == current_active_profile {
            log_error(format!(
                "Cannot delete currently active profile: {}. Switch to another profile first.",
                profile_name.0.log_color_error_highlight()
            ));
            bail!(NonSuccessfulExit);
        }

        Config::delete_profile(&profile_name, self.ctx.config_dir())?;

        log_warn_action(
            "Deleted",
            format!("profile {}", profile_name.0.log_color_highlight()),
        );

        Ok(())
    }
}
