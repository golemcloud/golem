// Copyright 2024 Golem Cloud
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

pub mod text;

use crate::model::{ComponentId, ComponentName};
use clap::{ArgMatches, Error, FromArgMatches};
use derive_more::{Display, FromStr, Into};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Serialize, Deserialize)]
pub struct AccountId {
    pub id: String,
}

impl AccountId {
    pub fn new(id: String) -> AccountId {
        AccountId { id }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Into)]
pub struct TokenId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Into, Serialize, Deserialize)]
pub struct ProjectId(pub Uuid);

impl Display for ProjectId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProjectRef {
    Id(ProjectId),
    Name(String),
    Default,
}

impl FromArgMatches for ProjectRef {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        ProjectRefArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: ProjectRefArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = ProjectRefArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for ProjectRef {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        ProjectRefArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct ProjectRefArgs {
    #[arg(short = 'P', long, conflicts_with = "project_name")]
    project_id: Option<Uuid>,

    #[arg(short = 'p', long, conflicts_with = "project_id")]
    project_name: Option<String>,
}

impl From<&ProjectRefArgs> for ProjectRef {
    fn from(value: &ProjectRefArgs) -> ProjectRef {
        if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        }
    }
}

impl From<&ProjectRef> for ProjectRefArgs {
    fn from(value: &ProjectRef) -> Self {
        match value {
            ProjectRef::Id(ProjectId(id)) => ProjectRefArgs {
                project_id: Some(*id),
                project_name: None,
            },
            ProjectRef::Name(name) => ProjectRefArgs {
                project_id: None,
                project_name: Some(name.clone()),
            },
            ProjectRef::Default => ProjectRefArgs {
                project_id: None,
                project_name: None,
            },
        }
    }
}

impl FromArgMatches for CloudComponentIdOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        CloudComponentIdOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: CloudComponentIdOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = CloudComponentIdOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for CloudComponentIdOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        CloudComponentIdOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        CloudComponentIdOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct CloudComponentIdOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "component_name", required = true)]
    component_id: Option<Uuid>,

    #[arg(short = 'c', long, conflicts_with = "component_id", required = true)]
    component_name: Option<String>,

    #[arg(
        short = 'P',
        long,
        conflicts_with = "project_name",
        conflicts_with = "component_id"
    )]
    project_id: Option<Uuid>,

    #[arg(
        short = 'p',
        long,
        conflicts_with = "project_id",
        conflicts_with = "component_id"
    )]
    project_name: Option<String>,
}

impl From<&CloudComponentIdOrNameArgs> for CloudComponentIdOrName {
    fn from(value: &CloudComponentIdOrNameArgs) -> CloudComponentIdOrName {
        let pr = if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        };

        if let Some(id) = value.component_id {
            CloudComponentIdOrName::Id(ComponentId(id))
        } else {
            CloudComponentIdOrName::Name(
                ComponentName(value.component_name.as_ref().unwrap().to_string()),
                pr,
            )
        }
    }
}

impl From<&CloudComponentIdOrName> for CloudComponentIdOrNameArgs {
    fn from(value: &CloudComponentIdOrName) -> CloudComponentIdOrNameArgs {
        match value {
            CloudComponentIdOrName::Id(ComponentId(id)) => CloudComponentIdOrNameArgs {
                component_id: Some(*id),
                component_name: None,
                project_id: None,
                project_name: None,
            },
            CloudComponentIdOrName::Name(ComponentName(name), pr) => {
                let (project_id, project_name) = match pr {
                    ProjectRef::Id(ProjectId(id)) => (Some(*id), None),
                    ProjectRef::Name(name) => (None, Some(name.to_string())),
                    ProjectRef::Default => (None, None),
                };

                CloudComponentIdOrNameArgs {
                    component_id: None,
                    component_name: Some(name.clone()),
                    project_id,
                    project_name,
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CloudComponentIdOrName {
    Id(ComponentId),
    Name(ComponentName, ProjectRef),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize)]
pub enum Role {
    Admin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => "Admin",
            Role::MarketingAdmin => "MarketingAdmin",
            Role::ViewProject => "ViewProject",
            Role::DeleteProject => "DeleteProject",
            Role::CreateProject => "CreateProject",
            Role::InstanceServer => "InstanceServer",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            "ViewProject" => Ok(Role::ViewProject),
            "DeleteProject" => Ok(Role::DeleteProject),
            "CreateProject" => Ok(Role::CreateProject),
            "InstanceServer" => Ok(Role::InstanceServer),
            _ => {
                let all = Role::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown role: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
pub enum ProjectAction {
    ViewComponent,
    CreateComponent,
    UpdateComponent,
    DeleteComponent,
    ViewWorker,
    CreateWorker,
    UpdateWorker,
    DeleteWorker,
    ViewProjectGrants,
    CreateProjectGrants,
    DeleteProjectGrants,
}

impl Display for ProjectAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ProjectAction::ViewComponent => "ViewComponent",
            ProjectAction::CreateComponent => "CreateComponent",
            ProjectAction::UpdateComponent => "UpdateComponent",
            ProjectAction::DeleteComponent => "DeleteComponent",
            ProjectAction::ViewWorker => "ViewWorker",
            ProjectAction::CreateWorker => "CreateWorker",
            ProjectAction::UpdateWorker => "UpdateWorker",
            ProjectAction::DeleteWorker => "DeleteWorker",
            ProjectAction::ViewProjectGrants => "ViewProjectGrants",
            ProjectAction::CreateProjectGrants => "CreateProjectGrants",
            ProjectAction::DeleteProjectGrants => "DeleteProjectGrants",
        };

        Display::fmt(s, f)
    }
}

impl FromStr for ProjectAction {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ViewComponent" => Ok(ProjectAction::ViewComponent),
            "CreateComponent" => Ok(ProjectAction::CreateComponent),
            "UpdateComponent" => Ok(ProjectAction::UpdateComponent),
            "DeleteComponent" => Ok(ProjectAction::DeleteComponent),
            "ViewWorker" => Ok(ProjectAction::ViewWorker),
            "CreateWorker" => Ok(ProjectAction::CreateWorker),
            "UpdateWorker" => Ok(ProjectAction::UpdateWorker),
            "DeleteWorker" => Ok(ProjectAction::DeleteWorker),
            "ViewProjectGrants" => Ok(ProjectAction::ViewProjectGrants),
            "CreateProjectGrants" => Ok(ProjectAction::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(ProjectAction::DeleteProjectGrants),
            _ => {
                let all = ProjectAction::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown action: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ProjectPolicyId(pub Uuid);
