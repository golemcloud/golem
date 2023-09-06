use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};

use std::str::FromStr;
use clap::{Arg, ArgMatches, Command, Error, FromArgMatches};
use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use derive_more::{Display, FromStr, Into};
use golem_client::account::AccountError;
use golem_client::component::ComponentError;
use golem_client::grant::GrantError;
use golem_client::instance::InstanceError;
use golem_client::login::LoginError;
use golem_client::project::ProjectError;
use golem_client::project_grant::ProjectGrantError;
use golem_client::project_policy::ProjectPolicyError;
use golem_client::token::TokenError;
use indoc::indoc;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Json(serde_json::value::Value),
    Str(String),
}

impl GolemResult {
    pub fn err(s: String) -> Result<GolemResult, GolemError> {
        Err(GolemError(s))
    }
}

pub trait PrintRes {
    fn println(&self, format: &Format) -> ();
}

impl<T> PrintRes for T
    where T: Serialize, {
    fn println(&self, format: &Format) -> () {
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GolemError(pub String);


impl From<AccountError> for GolemError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            AccountError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            AccountError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            AccountError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            AccountError::Status404 { message } => GolemError(format!("Not found: {message}")),
            AccountError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            AccountError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<TokenError> for GolemError {
    fn from(value: TokenError) -> Self {
        match value {
            TokenError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            TokenError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            TokenError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            TokenError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            TokenError::Status404 { message } => GolemError(format!("Not found: {message}")),
            TokenError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            TokenError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<ComponentError> for GolemError {
    fn from(value: ComponentError) -> Self {
        match value {
            ComponentError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            ComponentError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            ComponentError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            ComponentError::Status401 { error } => GolemError(format!("Unauthorized: {error}")),
            ComponentError::Status504 => GolemError(format!("Gateway Timeout")),
            ComponentError::Status404 { message } => GolemError(message),
            ComponentError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ComponentError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            ComponentError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
            ComponentError::Status409 { component_id } => GolemError(format!("{component_id} already exists")),
        }
    }
}

impl From<LoginError> for GolemError {
    fn from(value: LoginError) -> Self {
        match value {
            LoginError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            LoginError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            LoginError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            LoginError::Status400 { errors } => {
                let joined = errors.join(", ");
                GolemError(format!("Invalid request: {joined}"))
            }
            LoginError::Status403 { .. } => {
                let msg = indoc! {"
                    At the moment account creation is restricted.
                    None of your verified emails is whitelisted.
                    Please contact us to create an account.
                "};
                GolemError(msg.to_string())
            }
            LoginError::Status500 { error } => GolemError(format!("Internal server error on Login: {error}")),
            LoginError::Status401 { error } => GolemError(format!("External service call error on Login: {error}")),
        }
    }
}

impl From<ProjectError> for GolemError {
    fn from(value: ProjectError) -> Self {
        match value {
            ProjectError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            ProjectError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            ProjectError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            ProjectError::Status404 { message } => GolemError(format!("Not found: {message}")),
            ProjectError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            ProjectError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            ProjectError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ProjectError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<GrantError> for GolemError {
    fn from(value: GrantError) -> Self {
        match value {
            GrantError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            GrantError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            GrantError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            GrantError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            GrantError::Status404 { message } => GolemError(format!("Not found: {message}")),
            GrantError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            GrantError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<ProjectPolicyError> for GolemError {
    fn from(value: ProjectPolicyError) -> Self {
        match value {
            ProjectPolicyError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            ProjectPolicyError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            ProjectPolicyError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            ProjectPolicyError::Status404 { message } => GolemError(format!("Not found: {message}")),
            ProjectPolicyError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            ProjectPolicyError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            ProjectPolicyError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ProjectPolicyError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<ProjectGrantError> for GolemError {
    fn from(value: ProjectGrantError) -> Self {
        match value {
            ProjectGrantError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            ProjectGrantError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            ProjectGrantError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            ProjectGrantError::Status404 { message } => GolemError(format!("Not found: {message}")),
            ProjectGrantError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            ProjectGrantError::Status401 { message } => GolemError(format!("Unauthorized: {message}")),
            ProjectGrantError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ProjectGrantError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
        }
    }
}

impl From<InstanceError> for GolemError {
    fn from(value: InstanceError) -> Self {
        match value {
            InstanceError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            InstanceError::InvalidHeaderValue(err) => GolemError(format!("Unexpected invalid header value: {err}")),
            InstanceError::UnexpectedStatus(sc) => GolemError(format!("Unexpected status: {sc}")),
            InstanceError::Status504 => GolemError(format!("Gateway timeout")),
            InstanceError::Status404 { error } => GolemError(format!("Not found: {error}")),
            InstanceError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            InstanceError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            InstanceError::Status401 { error } => GolemError(format!("Unauthorized: {error}")),
            InstanceError::Status500 { golem_error } => GolemError(format!("Internal server error: {golem_error:?}")),
            InstanceError::Status409 { error } => GolemError(error),
        }
    }
}

impl Display for GolemError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let GolemError(s) = self;
        Display::fmt(s, f)
    }
}

impl Debug for GolemError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let GolemError(s) = self;
        Display::fmt(s, f)
    }
}

impl std::error::Error for GolemError {
    fn description(&self) -> &str {
        let GolemError(s) = self;

        s
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter)]
pub enum Format {
    Json,
    Yaml,
}

impl Display for Format {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
        };
        Display::fmt(&s, f)
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "json" => Ok(Format::Json),
            "yaml" => Ok(Format::Yaml),
            _ => {
                let all =
                    Format::iter()
                        .map(|x| format!("\"{x}\""))
                        .collect::<Vec<String>>()
                        .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct AccountId {
    pub id: String,
} // TODO: Validate


impl AccountId {
    pub fn new(id: String) -> AccountId {
        AccountId { id }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Into)]
pub struct TokenId(pub Uuid);

#[derive(Clone, PartialEq, Eq, Debug, Into)]
pub struct ProjectId(pub Uuid);

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
            ProjectRef::Id(ProjectId(id)) => {
                ProjectRefArgs { project_id: Some(id.clone()), project_name: None }
            }
            ProjectRef::Name(name) => {
                ProjectRefArgs { project_id: None, project_name: Some(name.clone()) }
            }
            ProjectRef::Default => {
                ProjectRefArgs { project_id: None, project_name: None }
            }
        }
    }
}

impl FromArgMatches for TemplateIdOrName {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, Error> {
        TemplateIdOrNameArgs::from_arg_matches(matches).map(|c| (&c).into())
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), Error> {
        let prc0: TemplateIdOrNameArgs = (&self.clone()).into();
        let mut prc = prc0.clone();
        let res = TemplateIdOrNameArgs::update_from_arg_matches(&mut prc, matches);
        *self = (&prc).into();
        res
    }
}

impl clap::Args for TemplateIdOrName {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        TemplateIdOrNameArgs::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        TemplateIdOrNameArgs::augment_args_for_update(cmd)
    }
}

#[derive(clap::Args, Debug, Clone)]
struct TemplateIdOrNameArgs {
    #[arg(short = 'C', long, conflicts_with = "template_name", required = true)]
    template_id: Option<Uuid>,

    #[arg(short, long, conflicts_with = "template_id", required = true)]
    template_name: Option<String>,

    #[arg(short = 'P', long, conflicts_with = "project_name", conflicts_with = "template_id")]
    project_id: Option<Uuid>,

    #[arg(short = 'p', long, conflicts_with = "project_id", conflicts_with = "template_id")]
    project_name: Option<String>,
}


impl From<&TemplateIdOrNameArgs> for TemplateIdOrName {
    fn from(value: &TemplateIdOrNameArgs) -> TemplateIdOrName {
        let pr = if let Some(id) = value.project_id {
            ProjectRef::Id(ProjectId(id))
        } else if let Some(name) = value.project_name.clone() {
            ProjectRef::Name(name)
        } else {
            ProjectRef::Default
        };

        if let Some(id) = value.template_id {
            TemplateIdOrName::Id(RawTemplateId(id))
        } else {
            TemplateIdOrName::Name(TemplateName(value.template_name.as_ref().unwrap().to_string()), pr)
        }
    }
}

impl From<&TemplateIdOrName> for TemplateIdOrNameArgs {
    fn from(value: &TemplateIdOrName) -> TemplateIdOrNameArgs {
        match value {
            TemplateIdOrName::Id(RawTemplateId(id)) => {
                TemplateIdOrNameArgs { template_id: Some(id.clone()), template_name: None, project_id: None, project_name: None }
            }
            TemplateIdOrName::Name(TemplateName(name), pr) => {
                let (project_id, project_name) = match pr {
                    ProjectRef::Id(ProjectId(id)) => {
                        (Some(*id), None)
                    }
                    ProjectRef::Name(name) => {
                        (None, Some(name.to_string()))
                    }
                    ProjectRef::Default => {
                        (None, None)
                    }
                };

                TemplateIdOrNameArgs { template_id: None, template_name: Some(name.clone()), project_id, project_name }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawTemplateId(pub Uuid);

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct TemplateName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TemplateIdOrName {
    Id(RawTemplateId),
    Name(TemplateName, ProjectRef),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, EnumIter, Serialize, Deserialize)]
pub enum Role {
    Admin,
    WhitelistAdmin,
    MarketingAdmin,
    ViewProject,
    DeleteProject,
    CreateProject,
    InstanceServer,
}

impl Display for Role {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::Admin => { "Admin" }
            Role::WhitelistAdmin => { "WhitelistAdmin" }
            Role::MarketingAdmin => { "MarketingAdmin" }
            Role::ViewProject => { "ViewProject" }
            Role::DeleteProject => { "DeleteProject" }
            Role::CreateProject => { "CreateProject" }
            Role::InstanceServer => { "InstanceServer" }
        };

        Display::fmt(s, f)
    }
}

impl FromStr for Role {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Admin" => Ok(Role::Admin),
            "WhitelistAdmin" => Ok(Role::WhitelistAdmin),
            "MarketingAdmin" => Ok(Role::MarketingAdmin),
            "ViewProject" => Ok(Role::ViewProject),
            "DeleteProject" => Ok(Role::DeleteProject),
            "CreateProject" => Ok(Role::CreateProject),
            "InstanceServer" => Ok(Role::InstanceServer),
            _ => {
                let all =
                    Role::iter()
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
    ViewTemplate,
    CreateTemplate,
    UpdateTemplate,
    DeleteTemplate,
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
            ProjectAction::ViewTemplate => "ViewTemplate",
            ProjectAction::CreateTemplate => "CreateTemplate",
            ProjectAction::UpdateTemplate => "UpdateTemplate",
            ProjectAction::DeleteTemplate => "DeleteTemplate",
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
            "ViewTemplate" => Ok(ProjectAction::ViewTemplate),
            "CreateTemplate" => Ok(ProjectAction::CreateTemplate),
            "UpdateTemplate" => Ok(ProjectAction::UpdateTemplate),
            "DeleteTemplate" => Ok(ProjectAction::DeleteTemplate),
            "ViewWorker" => Ok(ProjectAction::ViewWorker),
            "CreateWorker" => Ok(ProjectAction::CreateWorker),
            "UpdateWorker" => Ok(ProjectAction::UpdateWorker),
            "DeleteWorker" => Ok(ProjectAction::DeleteWorker),
            "ViewProjectGrants" => Ok(ProjectAction::ViewProjectGrants),
            "CreateProjectGrants" => Ok(ProjectAction::CreateProjectGrants),
            "DeleteProjectGrants" => Ok(ProjectAction::DeleteProjectGrants),
            _ => {
                let all =
                    ProjectAction::iter()
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

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct WorkerName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Serialize)]
pub struct InvocationKey(pub String); // TODO: Validate

#[derive(Clone)]
pub struct JsonValueParser;

impl TypedValueParser for JsonValueParser {
    type Value = serde_json::value::Value;

    fn parse_ref(&self, cmd: &Command, arg: Option<&Arg>, value: &OsStr) -> Result<Self::Value, Error> {
        let inner = StringValueParser::new();
        let val = inner.parse_ref(cmd, arg, value)?;
        let parsed = <serde_json::Value as std::str::FromStr>::from_str(&val);

        match parsed {
            Ok(value) => Ok(value),
            Err(serde_err) => {
                let mut err = clap::Error::new(ErrorKind::ValueValidation);
                if let Some(arg) = arg {
                    err.insert(ContextKind::InvalidArg, ContextValue::String(arg.to_string()));
                }
                err.insert(ContextKind::InvalidValue, ContextValue::String(format!("Invalid JSON value: {serde_err}")));
                Err(err)
            }
        }
    }
}