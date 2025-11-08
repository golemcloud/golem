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

use crate::log::LogColorize;
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use indoc::formatdoc;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, PartialEq, Debug)]
pub enum EnvironmentReference {
    Environment {
        environment_name: EnvironmentName,
    },
    ApplicationEnvironment {
        application_name: ApplicationName,
        environment_name: EnvironmentName,
    },
    AccountApplicationEnvironment {
        account_email: String,
        application_name: ApplicationName,
        environment_name: EnvironmentName,
        auto_create: bool,
    },
}

impl TryFrom<&str> for EnvironmentReference {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl TryFrom<String> for EnvironmentReference {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl FromStr for EnvironmentReference {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut segments = s.split("/").collect::<Vec<_>>();
        match segments.len() {
            1 => Ok(Self::Environment {
                environment_name: segments.pop().unwrap().parse()?,
            }),
            2 => Ok(Self::ApplicationEnvironment {
                application_name: segments.pop().unwrap().parse()?,
                environment_name: segments.pop().unwrap().parse()?,
            }),
            3 => Ok(Self::AccountApplicationEnvironment {
                account_email: segments.pop().unwrap().into(),
                application_name: segments.pop().unwrap().parse()?,
                environment_name: segments.pop().unwrap().parse()?,
                auto_create: false,
            }),
            _ => Err(formatdoc! {"
                Unknown format for environment: {}. Expected one of:
                - <ENVIRONMENT_NAME>
                - <APPLICATION_NAME>/<ENVIRONMENT_NAME>
                - <ACCOUNT_EMAIL>/<APPLICATION_NAME>/<ENVIRONMENT_NAME>
                ",
                s.log_color_highlight()
            }),
        }
    }
}

impl Display for EnvironmentReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Environment { environment_name } => write!(f, "{}", environment_name.0),
            Self::ApplicationEnvironment {
                application_name,
                environment_name,
            } => write!(f, "{}/{}", application_name.0, environment_name.0),
            Self::AccountApplicationEnvironment {
                account_email,
                environment_name,
                application_name,
                auto_create: _,
            } => write!(
                f,
                "{}/{}/{}",
                account_email, application_name.0, environment_name.0
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ResolvedEnvironmentIdentitySource {
    Reference(EnvironmentReference),
    DefaultFromManifest,
}

#[derive(Clone, Debug)]
pub struct ResolvedEnvironmentIdentity {
    pub source: ResolvedEnvironmentIdentitySource,

    pub account_id: AccountId,
    pub application_id: ApplicationId,
    pub application_name: ApplicationName,
    pub environment_id: EnvironmentId,
    pub environment_name: EnvironmentName,
}

#[derive(Clone, Debug, Copy)]
pub enum EnvironmentResolveMode {
    ManifestOnly, // The environment must be one of the ones defined in the manifest
    Any, // The environment can be one of the manifest ones, or any other "more" qualified reference
}

impl EnvironmentResolveMode {
    pub fn allowed(&self, environment: &EnvironmentReference) -> bool {
        match self {
            EnvironmentResolveMode::ManifestOnly => match environment {
                EnvironmentReference::Environment { .. } => true,
                EnvironmentReference::ApplicationEnvironment { .. } => false,
                EnvironmentReference::AccountApplicationEnvironment { .. } => false,
            },
            EnvironmentResolveMode::Any => true,
        }
    }
}
