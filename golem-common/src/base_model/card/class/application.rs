// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::{
    ClassPermissionPattern, PermissionClass, PermissionPattern, PolymorphicClassPermissionPattern,
    PolymorphicPermissionPattern, ResourcePattern, VerbPattern,
};
use crate::base_model::card::parsing::CardParseError;
use crate::model::card::owner::AccountOwnerPattern;
use combine::{EasyParser, Parser, eof, many1, satisfy};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationResourcePattern {
    Any,
    Application(ApplicationName),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct ApplicationName(pub String);

impl ResourcePattern for ApplicationResourcePattern {
    fn parse_resource(resource: &str) -> Result<Self, CardParseError> {
        if resource == "*" {
            Ok(ApplicationResourcePattern::Any)
        } else {
            parse_application_resource(resource).map_err(|_| CardParseError::InvalidResource {
                class: ApplicationClass::NAME.to_string(),
                resource: resource.to_string(),
            })
        }
    }

    fn subsumes(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Application(a), Self::Application(b)) => a == b,
            (Self::Application(_), Self::Any) => false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum ApplicationVerb {
    View,
    Create,
    Update,
    Delete,
    ListAllEnvironments,
}
impl VerbPattern for ApplicationVerb {
    fn parse_verb(verb: &str) -> Option<Self> {
        match verb {
            "view" => Some(Self::View),
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "list-all-environments" => Some(Self::ListAllEnvironments),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct ApplicationClass;

impl PermissionClass for ApplicationClass {
    type Verb = ApplicationVerb;
    type Owner = AccountOwnerPattern;
    type Resource = ApplicationResourcePattern;
    const NAME: &'static str = "application";

    fn into_permission(pattern: ClassPermissionPattern<Self>) -> PermissionPattern {
        PermissionPattern::Application(pattern)
    }

    fn into_polymorphic_permission(
        pattern: PolymorphicClassPermissionPattern<Self>,
    ) -> PolymorphicPermissionPattern {
        PolymorphicPermissionPattern::Application(pattern)
    }
}

fn parse_application_resource(resource: &str) -> Result<ApplicationResourcePattern, String> {
    application_resource()
        .skip(eof())
        .easy_parse(resource)
        .map(|(resource, _)| resource)
        .map_err(|_| resource.to_string())
}

fn application_resource<Input>() -> impl Parser<Input, Output = ApplicationResourcePattern>
where
    Input: combine::Stream<Token = char>,
{
    application_name().map(ApplicationResourcePattern::Application)
}

fn application_name<Input>() -> impl Parser<Input, Output = ApplicationName>
where
    Input: combine::Stream<Token = char>,
{
    many1(satisfy(|c: char| {
        c != ':' && c != '/' && !c.is_whitespace()
    }))
    .map(ApplicationName)
}
