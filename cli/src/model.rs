use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

use std::str::FromStr;
use derive_more::{Display, FromStr, Into};
use golem_client::model::{AccountEndpointError, LoginEndpointError};
use indoc::{indoc, formatdoc};
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

impl From<Option<LoginEndpointError>> for GolemError {
    fn from(value: Option<LoginEndpointError>) -> Self {
        match value {
            None => GolemError("Unexpected endpoint call error".to_string()),
            Some(err) => match err {
                LoginEndpointError::ArgValidation { errors } => GolemError(format!("Invalid Login API call: {}", errors.join(", "))),
                LoginEndpointError::NotWhitelisted { .. } => {
                    let msg = indoc! {r"
                        At the moment account creation is restricted.
                        None of your verified emails is whitelisted.
                        Please contact us to create an account.
                        "};

                    GolemError(msg.to_string())
                }
                LoginEndpointError::Internal { error } => GolemError(format!("Internal server error on Login: {error}")),
                LoginEndpointError::External { error } => GolemError(format!("External service call error on Login: {error}")),
            }
        }
    }
}

impl From<Option<AccountEndpointError>> for GolemError {
    fn from(value: Option<AccountEndpointError>) -> Self {
        match value {
            None => GolemError("Unexpected endpoint call error".to_string()),
            Some(err) => match err {
                AccountEndpointError::ArgValidation { errors } => GolemError(format!("Invalid API call: {}", errors.join(", "))),
                AccountEndpointError::Internal { error } => GolemError(format!("Internal server error: {error}")),
                AccountEndpointError::NotFound { message } => GolemError(format!("Not found: {message}")),
                AccountEndpointError::Unauthorized { message } => {
                    let msg = formatdoc!("
                      Authorisation error: {message}.
                      Consider removing configuration directory $$HOME/.golem
                    ");

                    GolemError(msg)
                }
            }
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
pub struct TokenId(Uuid);