use std::error::Error;
use std::fmt::{Debug, Display, Formatter};

use std::str::FromStr;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use serde::Serialize;

pub enum GolemResult {
    Ok(Box<dyn PrintRes>),
    Str(String),
    Err(String)
}

pub trait PrintRes {
    fn println(&self, format: &Format) -> ();
}

impl <T> PrintRes for T
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
