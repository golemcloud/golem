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

pub mod config;

use crate::command::profile::ProfileSubcommand;
use crate::command_handler::Handlers;
use crate::config::{
    CloudProfile, Config, NamedProfile, OssProfile, Profile, ProfileConfig, ProfileKind,
    ProfileName, DEFAULT_OSS_URL,
};
use crate::context::Context;
use crate::error::NonSuccessfulExit;
use crate::log::{log_action, log_warn_action, LogColorize};
use crate::model::text::fmt::log_error;
use crate::model::{Format, ProfileView};
use anyhow::bail;
use std::sync::Arc;
use url::Url;

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
                profile_kind,
                name,
                set_active,
                component_url,
                worker_url,
                cloud_url,
                default_format,
                allow_insecure,
            } => self.cmd_new(
                profile_kind,
                name,
                set_active,
                component_url,
                worker_url,
                cloud_url,
                default_format,
                allow_insecure,
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
        kind: ProfileKind,
        name: Option<ProfileName>,
        set_active: bool,
        component_url: Option<Url>,
        worker_url: Option<Url>,
        cloud_url: Option<Url>,
        default_format: Format,
        allow_insecure: bool,
    ) -> anyhow::Result<()> {
        let (name, profile, set_active) = match name {
            Some(name) => {
                if name.is_builtin() {
                    log_error("The requested profile name {} is a builtin profile. Please choose another profile name!");
                    bail!(NonSuccessfulExit);
                }

                if cloud_url.is_some() {
                    log_error("cloud_url is not allowed for OSS profile.");
                    bail!(NonSuccessfulExit);
                }

                let profile = match kind {
                    ProfileKind::Oss => Profile::Golem(OssProfile {
                        url: component_url.unwrap_or(Url::parse(DEFAULT_OSS_URL)?),
                        worker_url,
                        allow_insecure,
                        config: ProfileConfig { default_format },
                    }),
                    ProfileKind::Cloud => Profile::GolemCloud(CloudProfile {
                        custom_url: component_url,
                        custom_cloud_url: None,
                        custom_worker_url: None,
                        allow_insecure,
                        config: Default::default(),
                        auth: None,
                    }),
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

        let profiles = config
            .profiles
            .into_iter()
            .map(|(name, profile)| {
                ProfileView::from_profile(&default_profile_name, NamedProfile { name, profile })
            })
            .collect::<Vec<_>>();

        self.ctx.log_handler().log_view(&profiles);

        Ok(())
    }

    fn cmd_switch(&self, profile_name: ProfileName) -> anyhow::Result<()> {
        Config::set_active_profile_name(profile_name, self.ctx.config_dir())?;
        Ok(())
    }

    fn cmd_get(&self, profile_name: Option<ProfileName>) -> anyhow::Result<()> {
        let active_profile = Config::get_active_profile(self.ctx.config_dir(), None)?;
        let active_profile_name = active_profile.name.clone();

        let profile = match profile_name {
            Some(profile_name) => {
                match Config::get_profile(&profile_name, self.ctx.config_dir())? {
                    Some(profile) => NamedProfile {
                        name: profile_name,
                        profile,
                    },
                    None => {
                        log_error(format!(
                            "Profile {} not found",
                            profile_name.0.log_color_error_highlight()
                        ));
                        bail!(NonSuccessfulExit);
                    }
                }
            }
            None => active_profile,
        };

        self.ctx
            .log_handler()
            .log_view(&ProfileView::from_profile(&active_profile_name, profile));

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

        Config::delete_profile(&profile_name, self.ctx.config_dir())?;

        log_warn_action(
            "Deleted",
            format!("profile {}", profile_name.0.log_color_highlight()),
        );

        Ok(())
    }
}
