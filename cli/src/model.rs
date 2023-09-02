use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

use std::str::FromStr;
use derive_more::{Display, FromStr, Into};
use golem_client::account::AccountError;
use golem_client::component::ComponentError;
use golem_client::login::LoginError;
use golem_client::project::ProjectError;
use golem_client::token::TokenError;
use indoc::indoc;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use serde::Serialize;
use uuid::Uuid;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
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
            AccountError::InvalidHeaderValue(err) =>  GolemError(format!("Unexpected invalid header value: {err}")),
            AccountError::UnexpectedStatus(sc) =>  GolemError(format!("Unexpected status: {sc}")),
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
            TokenError::InvalidHeaderValue(err) =>  GolemError(format!("Unexpected invalid header value: {err}")),
            TokenError::UnexpectedStatus(sc) =>  GolemError(format!("Unexpected status: {sc}")),
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
            ComponentError::InvalidHeaderValue(err) =>  GolemError(format!("Unexpected invalid header value: {err}")),
            ComponentError::UnexpectedStatus(sc) =>  GolemError(format!("Unexpected status: {sc}")),
            ComponentError::Status504 => GolemError(format!("Gateway Timeout")),
            ComponentError::Status404 { message } => GolemError(message),
            ComponentError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ComponentError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            },
            ComponentError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
            ComponentError::Status409 { component_id } => GolemError(format!("{component_id} already exists")),
        }
    }
}

impl From<LoginError> for GolemError {
    fn from(value: LoginError) -> Self {
        match value {
            LoginError::RequestFailure(err) => GolemError(format!("Unexpected request failure: {err}")),
            LoginError::InvalidHeaderValue(err) =>  GolemError(format!("Unexpected invalid header value: {err}")),
            LoginError::UnexpectedStatus(sc) =>  GolemError(format!("Unexpected status: {sc}")),
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
            ProjectError::InvalidHeaderValue(err) =>  GolemError(format!("Unexpected invalid header value: {err}")),
            ProjectError::UnexpectedStatus(sc) =>  GolemError(format!("Unexpected status: {sc}")),
            ProjectError::Status404 { message } => GolemError(format!("Not found: {message}")),
            ProjectError::Status400 { errors } => {
                let msg = errors.join(", ");
                GolemError(format!("Invalid API call: {msg}"))
            }
            ProjectError::Status403 { error } => GolemError(format!("Limit Exceeded: {error}")),
            ProjectError::Status500 { error } => GolemError(format!("Internal server error: {error}")),
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

impl Error for GolemError {
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawComponentId(pub Uuid);

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct ComponentName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ComponentIdOrName {
    Id(RawComponentId),
    Name(ComponentName, ProjectRef),
}