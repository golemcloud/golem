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

use crate::uri::cloud::url::{AccountUrl, ProjectUrl};
use crate::uri::cloud::{ACCOUNT_TYPE_NAME, PROJECT_TYPE_NAME};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use url::Url;

pub mod cloud;
pub mod macros;
pub mod oss;

pub const COMPONENT_TYPE_NAME: &str = "component";
pub const WORKER_TYPE_NAME: &str = "worker";
pub const API_DEFINITION_TYPE_NAME: &str = "api-definition";
pub const API_DEPLOYMENT_TYPE_NAME: &str = "api-deployment";

/// Reference to a Golem resource
/// Resolving such reference might require additional queries to multiple Golem services.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum GolemUri {
    URL(GolemUrl),
    URN(GolemUrn),
}

#[derive(Debug, Clone)]
pub enum GolemUriParseError {
    URN(GolemUrnParseError),
    URL(GolemUrlParseError),
}

impl Display for GolemUriParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUriParseError::URN(err) => {
                write!(f, "Failed to parse Golem URI: {err}")
            }
            GolemUriParseError::URL(err) => {
                write!(f, "Failed to parse Golem URI: {err}")
            }
        }
    }
}

impl FromStr for GolemUri {
    type Err = GolemUriParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("urn:") {
            let urn = GolemUrn::from_str(s).map_err(GolemUriParseError::URN)?;
            Ok(GolemUri::URN(urn))
        } else {
            let url = GolemUrl::from_str(s).map_err(GolemUriParseError::URL)?;
            Ok(GolemUri::URL(url))
        }
    }
}

impl Display for GolemUri {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUri::URL(url) => {
                write!(f, "{}", url)
            }
            GolemUri::URN(urn) => {
                write!(f, "{}", urn)
            }
        }
    }
}

impl Serialize for GolemUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GolemUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Fully resolved Golem resource name
/// This name is enough to locate Golem resource in a service without additional queries to different Golem services.
///
/// User should not try to create such URN manually.
///
/// All user facing APIs should return URN for created resources.
/// All user facing APIs should return URN for resource search.
/// User facing APIs should avoid resource IDs - use URN instead.
///
/// The other way to get resource GolemUrn is to resolve a GolemUrl.
///
/// URN format is `urn:{resource_type}:{resource_name}`
/// `resource_type` is case-insensitive
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct GolemUrn {
    pub resource_type: String,
    pub resource_name: String,
}

impl Display for GolemUrn {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "urn:{}:{}", self.resource_type, self.resource_name)
    }
}

#[derive(Debug, Copy, Clone)]
pub enum GolemUrnParseError {
    NoUrnPrefix,
    NoResourceName,
    EmptyResourceType,
    InvalidResourceType(char),
}

impl Display for GolemUrnParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUrnParseError::NoUrnPrefix => {
                write!(f, "Failed to parse Golem URN: 'urn:' prefix expected.")
            }
            GolemUrnParseError::NoResourceName => {
                write!(f, "Failed to parse Golem URN: resource name expected.")
            }
            GolemUrnParseError::EmptyResourceType => {
                write!(f, "Failed to parse Golem URN: empty resource type.")
            }
            GolemUrnParseError::InvalidResourceType(c) => {
                write!(
                    f,
                    "Failed to parse Golem URN: unexpected character '{c}' in resource type."
                )
            }
        }
    }
}

impl FromStr for GolemUrn {
    type Err = GolemUrnParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(no_prefix) = s.strip_prefix("urn:") {
            if let Some((resource_type, resource_name)) = no_prefix.split_once(':') {
                if resource_type.is_empty() {
                    Err(GolemUrnParseError::EmptyResourceType)
                } else if resource_name.is_empty() {
                    Err(GolemUrnParseError::NoResourceName)
                } else {
                    Ok(GolemUrn {
                        resource_type: resource_type.to_lowercase(),
                        resource_name: resource_name.to_string(),
                    })
                }
            } else {
                Err(GolemUrnParseError::NoResourceName)
            }
        } else {
            Err(GolemUrnParseError::NoUrnPrefix)
        }
    }
}

#[derive(Debug, Clone)]
pub enum GolemUrnTransformError {
    InvalidName {
        target_type: &'static str,
        err: String,
    },
    UnexpectedType {
        expected_types: Vec<&'static str>,
        actual_type: String,
    },
    UrnParseError {
        err: GolemUrnParseError,
    },
}

impl Error for GolemUrnTransformError {}

impl GolemUrnTransformError {
    pub fn invalid_name(target_type: &'static str, err: String) -> GolemUrnTransformError {
        GolemUrnTransformError::InvalidName { target_type, err }
    }
}

impl Display for GolemUrnTransformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUrnTransformError::InvalidName { target_type, err } => {
                write!(f, "Failed to parse URN of type {target_type}: {err}")
            }
            GolemUrnTransformError::UnexpectedType {
                expected_types,
                actual_type,
            } => {
                if expected_types.len() == 1 {
                    let expected_type = expected_types.first().unwrap();
                    write!(
                        f,
                        "URN of type {expected_type} expected, but got {actual_type}"
                    )
                } else {
                    let expected = expected_types.join(", ");

                    write!(f, "Unexpected resource type in URN: {actual_type}. Expected one of: {expected}")
                }
            }
            GolemUrnTransformError::UrnParseError { err } => {
                write!(f, "Failed to parse URN: {err}")
            }
        }
    }
}

pub trait TypedGolemUrn {
    fn resource_type() -> &'static str;
    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError>
    where
        Self: Sized;
    fn to_name(&self) -> String;
}

pub fn try_from_golem_urn<T: TypedGolemUrn>(urn: &GolemUrn) -> Result<T, GolemUrnTransformError> {
    let expected_type = T::resource_type();

    if urn.resource_type != expected_type {
        Err(GolemUrnTransformError::UnexpectedType {
            expected_types: vec![expected_type],
            actual_type: urn.resource_type.to_string(),
        })
    } else {
        T::try_from_name(&urn.resource_name)
    }
}

impl GolemUrn {
    /// Create GolemUrn from type and name
    ///
    /// Should be used for `From` implementations - see `urn_from_example` test
    pub fn try_from_parts<N: Into<String>>(
        resource_type: &str,
        resource_name: N,
    ) -> Result<Self, GolemUrnParseError> {
        validate_resource_type(resource_type).map_err(GolemUrnParseError::InvalidResourceType)?;

        Ok(Self {
            resource_type: resource_type.to_string(),
            resource_name: resource_name.into(),
        })
    }
}

impl Serialize for GolemUrn {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GolemUrn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

fn validate_resource_type(resource_type: &str) -> Result<(), char> {
    for c in resource_type.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '-' | '.' => {}
            _ => {
                return Err(c);
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub enum GolemUrlTransformError {
    InvalidPath {
        target_type: &'static str,
        err: String,
    },
    InvalidQuery {
        target_type: &'static str,
        key: &'static str,
        err: String,
    },
    UnexpectedQuery {
        target_type: &'static str,
        key: String,
    },
    UnexpectedType {
        expected_types: Vec<&'static str>,
        actual_type: String,
    },
    UrlParseError {
        err: GolemUrlParseError,
    },
}

impl Error for GolemUrlTransformError {}

impl Display for GolemUrlTransformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUrlTransformError::InvalidPath { target_type, err } => {
                write!(
                    f,
                    "Failed to parse URL of type {target_type}, invalid path: {err}"
                )
            }
            GolemUrlTransformError::InvalidQuery {
                target_type,
                key,
                err,
            } => {
                write!(
                    f,
                    "Failed to parse URL of type {target_type}, invalid query key '{key}': {err}"
                )
            }
            GolemUrlTransformError::UnexpectedQuery { target_type, key } => {
                write!(
                    f,
                    "Failed to parse URL of type {target_type}: unexpected query key '{key}'"
                )
            }
            GolemUrlTransformError::UnexpectedType {
                expected_types,
                actual_type,
            } => {
                if expected_types.len() == 1 {
                    let expected_type = expected_types.first().unwrap();
                    write!(
                        f,
                        "URL of type {expected_type} expected, but got {actual_type}"
                    )
                } else {
                    let expected = expected_types.join(", ");

                    write!(f, "Unexpected resource type in URL: {actual_type}. Expected one of: {expected}")
                }
            }
            GolemUrlTransformError::UrlParseError { err } => {
                write!(f, "Failed to parse URL: {err}")
            }
        }
    }
}

impl GolemUrlTransformError {
    pub fn invalid_path<S: Into<String>>(target_type: &'static str, err: S) -> Self {
        Self::InvalidPath {
            target_type,
            err: err.into(),
        }
    }

    pub fn invalid_query(target_type: &'static str, key: &'static str, err: String) -> Self {
        Self::InvalidQuery {
            target_type,
            key,
            err,
        }
    }
}

pub trait TypedGolemUrl {
    fn resource_type() -> &'static str;
    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized;
    fn to_parts(&self) -> (String, Option<String>);

    fn invalid_path<S: Into<String>>(err: S) -> GolemUrlTransformError {
        GolemUrlTransformError::invalid_path(Self::resource_type(), err)
    }

    fn expect_path1(path: &str) -> Result<String, GolemUrlTransformError> {
        let path = path
            .strip_prefix('/')
            .ok_or(Self::invalid_path("path is not started with '/'"))?;

        if path.contains('/') {
            Err(Self::invalid_path(format!(
                "1 segment expected, but got {} segments",
                path.split('/').count()
            )))
        } else {
            Ok(urldecode(path))
        }
    }

    fn make_path1(elem1: &str) -> String {
        format!("/{}", urlencode(elem1))
    }

    fn expect_path2(path: &str) -> Result<(String, String), GolemUrlTransformError> {
        let path = path
            .strip_prefix('/')
            .ok_or(Self::invalid_path("path is not started with '/'"))?;

        let segments = path.split('/').collect::<Vec<_>>();

        if segments.len() != 2 {
            Err(Self::invalid_path(format!(
                "2 segments expected, but got {} segments",
                segments.len()
            )))
        } else {
            let segment1 = segments.first().unwrap();
            let segment2 = segments.get(1).unwrap();

            Ok((urldecode(segment1), urldecode(segment2)))
        }
    }

    fn expect_path3(path: &str) -> Result<(String, String, String), GolemUrlTransformError> {
        let path = path
            .strip_prefix('/')
            .ok_or(Self::invalid_path("path is not started with '/'"))?;

        let segments = path.split('/').collect::<Vec<_>>();

        if segments.len() != 3 {
            Err(Self::invalid_path(format!(
                "3 segments expected, but got {} segments",
                segments.len()
            )))
        } else {
            let segment1 = segments.first().unwrap();
            let segment2 = segments.get(1).unwrap();
            let segment3 = segments.get(2).unwrap();

            Ok((
                urldecode(segment1),
                urldecode(segment2),
                urldecode(segment3),
            ))
        }
    }

    fn make_path2(elem1: &str, elem2: &str) -> String {
        format!("/{}/{}", urlencode(elem1), urlencode(elem2))
    }

    fn make_path3(elem1: &str, elem2: &str, elem3: &str) -> String {
        format!(
            "/{}/{}/{}",
            urlencode(elem1),
            urlencode(elem2),
            urlencode(elem3)
        )
    }

    fn expect_empty_query(
        query: Option<&str>,
        cloud_keys: &'static [&'static str],
    ) -> Result<(), GolemUrlTransformError> {
        let pairs = url::form_urlencoded::parse(query.unwrap_or("").as_bytes()).collect::<Vec<_>>();

        if let Some((key, _)) = pairs.first() {
            let cloud_key = cloud_keys.iter().find(|&&k| k == key);

            if let Some(&key) = cloud_key {
                Err(GolemUrlTransformError::InvalidQuery {
                    target_type: Self::resource_type(),
                    key,
                    err: "Cloud context is not supported in current profile".to_string(),
                })
            } else {
                Err(GolemUrlTransformError::UnexpectedQuery {
                    target_type: Self::resource_type(),
                    key: key.to_string(),
                })
            }
        } else {
            Ok(())
        }
    }

    fn expect_query(
        query: Option<&str>,
        expected_keys: &'static [&'static str],
    ) -> Result<HashMap<&'static str, String>, GolemUrlTransformError> {
        let pairs = url::form_urlencoded::parse(query.unwrap_or("").as_bytes()).collect::<Vec<_>>();

        let mut res = HashMap::new();

        for (k, v) in pairs {
            match expected_keys.iter().find(|ek| k == **ek) {
                Some(ek) => {
                    let _ = res.insert(*ek, v.to_string());
                }
                None => {
                    return Err(GolemUrlTransformError::UnexpectedQuery {
                        target_type: Self::resource_type(),
                        key: k.to_string(),
                    })
                }
            }
        }

        Ok(res)
    }

    fn expect_project_query(
        query: Option<&str>,
    ) -> Result<Option<ProjectUrl>, GolemUrlTransformError> {
        let mut query = Self::expect_query(query, &[ACCOUNT_TYPE_NAME, PROJECT_TYPE_NAME])?;

        let account = query
            .remove(&ACCOUNT_TYPE_NAME)
            .map(|account_name| AccountUrl { name: account_name });

        let project = query
            .remove(&PROJECT_TYPE_NAME)
            .map(|project_name| ProjectUrl {
                name: project_name,
                account,
            });

        Ok(project)
    }
}

pub fn try_from_golem_url<T: TypedGolemUrl>(url: &GolemUrl) -> Result<T, GolemUrlTransformError> {
    let expected_type = T::resource_type();

    if url.resource_type != expected_type {
        Err(GolemUrlTransformError::UnexpectedType {
            expected_types: vec![expected_type],
            actual_type: url.resource_type.to_string(),
        })
    } else {
        T::try_from_parts(&url.path, url.query.as_deref())
    }
}

/// Human-readable location for a Golem resource
/// Resolving such reference requires additional queries to Golem services.
///
/// Golem URL is a subset or URL
///
/// You can use query parameters for optional context.
///
/// URL authority is not supported.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct GolemUrl {
    pub resource_type: String,
    pub path: String,
    pub query: Option<String>,
}

impl Display for GolemUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.resource_type, self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum GolemUrlParseError {
    UnexpectedAuthority,
    NoResourceType,
    InvalidResourceType(char),
    InvalidUrl(url::ParseError),
}

impl Error for GolemUrlParseError {
    fn cause(&self) -> Option<&dyn Error> {
        match self {
            GolemUrlParseError::InvalidUrl(err) => Some(err),
            _ => None,
        }
    }
}

impl From<url::ParseError> for GolemUrlParseError {
    fn from(value: url::ParseError) -> Self {
        GolemUrlParseError::InvalidUrl(value)
    }
}

impl Display for GolemUrlParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUrlParseError::UnexpectedAuthority => {
                write!(f, "Failed to parse Golem URL: authority is not supported.")
            }
            GolemUrlParseError::NoResourceType => {
                write!(f, "Failed to parse Golem URL: resource type expected.")
            }
            GolemUrlParseError::InvalidResourceType(c) => write!(
                f,
                "Failed to parse Golem URL: unexpected character '{c}' in resource type."
            ),
            GolemUrlParseError::InvalidUrl(err) => write!(f, "Failed to parse Golem URL: {err}"),
        }
    }
}

impl TryFrom<Url> for GolemUrl {
    type Error = GolemUrlParseError;

    fn try_from(value: Url) -> Result<Self, Self::Error> {
        if value.has_authority() && !value.authority().is_empty() {
            return Err(GolemUrlParseError::UnexpectedAuthority);
        }

        let resource_type = value.scheme();
        let path = value.path();
        let query = value.query();

        GolemUrl::from_parts(resource_type, path, query)
    }
}

impl FromStr for GolemUrl {
    type Err = GolemUrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Url::try_from(s)?.try_into()
    }
}

impl GolemUrl {
    pub fn from_parts<P: Into<String>, Q: Into<String>>(
        resource_type: &str,
        path: P,
        query: Option<Q>,
    ) -> Result<Self, GolemUrlParseError> {
        validate_resource_type(resource_type).map_err(GolemUrlParseError::InvalidResourceType)?;

        Ok(Self {
            resource_type: resource_type.to_string(),
            path: path.into(),
            query: query.map(|q| q.into()),
        })
    }
}

pub fn urldecode(s: &str) -> String {
    fn join(k: Cow<str>, v: Cow<str>) -> String {
        if v.is_empty() {
            k.into_owned()
        } else {
            [k, v].join("=")
        }
    }

    url::form_urlencoded::parse(s.as_bytes())
        .map(|(k, v)| join(k, v))
        .collect()
}

pub fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Serialize for GolemUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for GolemUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::uri::{GolemUri, GolemUrl, GolemUrn};
    use std::str::FromStr;
    use url::Url;

    #[test]
    pub fn parse_from_json() {
        let urn_json = "\"urn:type:name\"";
        let url_json = "\"type:///path&k=v\"";

        let urn: GolemUrn = serde_json::from_str(urn_json).unwrap();
        let url: GolemUrl = serde_json::from_str(url_json).unwrap();
        let uri: GolemUri = serde_json::from_str(urn_json).unwrap();

        assert_eq!(urn.to_string(), "urn:type:name");
        assert_eq!(uri.to_string(), "urn:type:name");
        assert_eq!(url.to_string(), "type:///path&k=v");

        let urn_res = serde_json::to_string(&urn).unwrap();
        let url_res = serde_json::to_string(&url).unwrap();
        let uri_res = serde_json::to_string(&uri).unwrap();

        assert_eq!(urn_res, urn_json);
        assert_eq!(uri_res, urn_json);
        assert_eq!(url_res, url_json);
    }

    #[test]
    pub fn parse_uri() {
        let uri_urn = GolemUri::from_str("urn:type:name").unwrap();
        let uri_url = GolemUri::from_str("type:///path&k=v").unwrap();
        assert_eq!(uri_urn.to_string(), "urn:type:name");
        assert_eq!(uri_url.to_string(), "type:///path&k=v");
    }

    #[test]
    pub fn parse_urn() {
        let res = GolemUrn::from_str("urn:my-type:my: name").unwrap();
        assert_eq!(res.to_string(), "urn:my-type:my: name");
    }

    #[test]
    pub fn parse_urn_case_insensitive() {
        let res = GolemUrn::from_str("urn:My-Type:my name").unwrap();
        assert_eq!(res.to_string(), "urn:my-type:my name");
    }

    #[test]
    pub fn urn_from_example() {
        struct CoolUrn(String);
        impl From<CoolUrn> for GolemUrn {
            fn from(value: CoolUrn) -> Self {
                GolemUrn::try_from_parts("cool", value.0).unwrap()
            }
        }

        assert_eq!(
            GolemUrn::from(CoolUrn("abc".to_string())).to_string(),
            "urn:cool:abc"
        );
    }

    #[test]
    pub fn golem_url_from_url() {
        let url = Url::parse("worker:///679ae459-8700-41d9-920c-7e2887459c94/worker1").unwrap();
        let golem_url: GolemUrl = url.try_into().unwrap();

        assert_eq!(
            golem_url.to_string(),
            "worker:///679ae459-8700-41d9-920c-7e2887459c94/worker1"
        );
    }

    #[test]
    pub fn url_query() {
        let golem_url = GolemUrl::from_str("cool:///id&context=value").unwrap();

        assert_eq!(golem_url.to_string(), "cool:///id&context=value");
    }

    #[test]
    pub fn url_no_authority() {
        let res = GolemUrl::from_str("cool://localhost/id");

        assert!(res.is_err());
    }
}
