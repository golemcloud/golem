use std::ffi::OsStr;
use std::fmt::{Debug, Display, Formatter};
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{StringValueParser, TypedValueParser};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use clap::{Arg, ArgMatches, Command, Error, FromArgMatches};
use derive_more::{Display, FromStr};
use golem_examples::model::{Example, ExampleName, GuestLanguage, GuestLanguageTier};
use serde::Serialize;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
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
    fn println(&self, format: &Format);
}

impl<T> PrintRes for T
where
    T: Serialize,
{
    fn println(&self, format: &Format) {
        match format {
            Format::Json => println!("{}", serde_json::to_string_pretty(self).unwrap()),
            Format::Yaml => println!("{}", serde_yaml::to_string(self).unwrap()),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GolemError(pub String);

impl From<reqwest::Error> for GolemError {
    fn from(error: reqwest::Error) -> Self {
        GolemError(format!("Unexpected reqwest error: {error}"))
    }
}

impl<T: crate::clients::errors::ResponseContentErrorMapper> From<golem_client::Error<T>>
    for GolemError
{
    fn from(value: golem_client::Error<T>) -> Self {
        match value {
            golem_client::Error::Reqwest(error) => GolemError::from(error),
            golem_client::Error::Serde(error) => {
                GolemError(format!("Unexpected serde error: {error}"))
            }
            golem_client::Error::Item(data) => {
                let error_str = crate::clients::errors::ResponseContentErrorMapper::map(data);
                GolemError(format!("Response error: {error_str}"))
            }
            golem_client::Error::Unexpected { code, data } => {
                match String::from_utf8(Vec::from(data)) {
                    Ok(data_string) => GolemError(format!(
                        "Unexpected http error. Code: {code}, content: {data_string}."
                    )),
                    Err(_) => GolemError(format!(
                        "Unexpected http error. Code: {code}, can't parse content as string."
                    )),
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
                let all = Format::iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<String>>()
                    .join(", ");
                Err(format!("Unknown format: {s}. Expected one of {all}"))
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
    #[arg(short = 'T', long, conflicts_with = "template_name", required = true)]
    template_id: Option<Uuid>,

    #[arg(short, long, conflicts_with = "template_id", required = true)]
    template_name: Option<String>,
}

impl From<&TemplateIdOrNameArgs> for TemplateIdOrName {
    fn from(value: &TemplateIdOrNameArgs) -> TemplateIdOrName {
        if let Some(id) = value.template_id {
            TemplateIdOrName::Id(RawTemplateId(id))
        } else {
            TemplateIdOrName::Name(TemplateName(
                value.template_name.as_ref().unwrap().to_string(),
            ))
        }
    }
}

impl From<&TemplateIdOrName> for TemplateIdOrNameArgs {
    fn from(value: &TemplateIdOrName) -> TemplateIdOrNameArgs {
        match value {
            TemplateIdOrName::Id(RawTemplateId(id)) => TemplateIdOrNameArgs {
                template_id: Some(*id),
                template_name: None,
            },
            TemplateIdOrName::Name(TemplateName(name)) => TemplateIdOrNameArgs {
                template_id: None,
                template_name: Some(name.clone()),
            },
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
    Name(TemplateName),
}

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr)]
pub struct WorkerName(pub String); // TODO: Validate

#[derive(Clone, PartialEq, Eq, Debug, Display, FromStr, Serialize)]
pub struct InvocationKey(pub String); // TODO: Validate

#[derive(Clone)]
pub struct JsonValueParser;

impl TypedValueParser for JsonValueParser {
    type Value = serde_json::value::Value;

    fn parse_ref(
        &self,
        cmd: &Command,
        arg: Option<&Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, Error> {
        let inner = StringValueParser::new();
        let val = inner.parse_ref(cmd, arg, value)?;
        let parsed = <serde_json::Value as std::str::FromStr>::from_str(&val);

        match parsed {
            Ok(value) => Ok(value),
            Err(serde_err) => {
                let mut err = clap::Error::new(ErrorKind::ValueValidation);
                if let Some(arg) = arg {
                    err.insert(
                        ContextKind::InvalidArg,
                        ContextValue::String(arg.to_string()),
                    );
                }
                err.insert(
                    ContextKind::InvalidValue,
                    ContextValue::String(format!("Invalid JSON value: {serde_err}")),
                );
                Err(err)
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct ExampleDescription {
    pub name: ExampleName,
    pub language: GuestLanguage,
    pub description: String,
    pub tier: GuestLanguageTier,
}

impl ExampleDescription {
    pub fn from_example(example: &Example) -> Self {
        Self {
            name: example.name.clone(),
            language: example.language.clone(),
            description: example.description.clone(),
            tier: example.language.tier(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum PathBufOrStdin {
    Path(PathBuf),
    Stdin,
}

impl FromStr for PathBufOrStdin {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "-" {
            Ok(PathBufOrStdin::Stdin)
        } else {
            Ok(PathBufOrStdin::Path(PathBuf::from_str(s)?))
        }
    }
}
