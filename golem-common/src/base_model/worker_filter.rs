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

use crate::base_model::component::ComponentRevision;
use crate::base_model::{AgentStatus, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentNameFilter {
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentNameFilter {
    pub fn new(comparator: StringFilterComparator, value: String) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentNameFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "name {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentStatusFilter {
    pub comparator: FilterComparator,
    pub value: AgentStatus,
}

impl AgentStatusFilter {
    pub fn new(comparator: FilterComparator, value: AgentStatus) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentStatusFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "status == {:?}", self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentRevisionFilter {
    pub comparator: FilterComparator,
    pub value: ComponentRevision,
}

impl AgentRevisionFilter {
    pub fn new(comparator: FilterComparator, value: ComponentRevision) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentRevisionFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "version {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentCreatedAtFilter {
    pub comparator: FilterComparator,
    pub value: Timestamp,
}

impl AgentCreatedAtFilter {
    pub fn new(comparator: FilterComparator, value: Timestamp) -> Self {
        Self { comparator, value }
    }
}

impl Display for AgentCreatedAtFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "created_at {} {}", self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct AgentEnvFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentEnvFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for AgentEnvFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "env.{} {} {}", self.name, self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigVarsFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl AgentConfigVarsFilter {
    pub fn new(name: String, comparator: StringFilterComparator, value: String) -> Self {
        Self {
            name,
            comparator,
            value,
        }
    }
}

impl Display for AgentConfigVarsFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "config.{} {} {}", self.name, self.comparator, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentAndFilter {
    pub filters: Vec<AgentFilter>,
}

impl AgentAndFilter {
    pub fn new(filters: Vec<AgentFilter>) -> Self {
        Self { filters }
    }
}

impl Display for AgentAndFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({})",
            self.filters
                .iter()
                .map(|f| f.clone().to_string())
                .collect::<Vec<String>>()
                .join(" AND ")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentOrFilter {
    pub filters: Vec<AgentFilter>,
}

impl AgentOrFilter {
    pub fn new(filters: Vec<AgentFilter>) -> Self {
        Self { filters }
    }
}

impl Display for AgentOrFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({})",
            self.filters
                .iter()
                .map(|f| f.clone().to_string())
                .collect::<Vec<String>>()
                .join(" OR ")
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentNotFilter {
    pub filter: Box<AgentFilter>,
}

impl AgentNotFilter {
    pub fn new(filter: AgentFilter) -> Self {
        Self {
            filter: Box::new(filter),
        }
    }
}

impl Display for AgentNotFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NOT ({})", self.filter)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", desert(evolution()))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum AgentFilter {
    Name(AgentNameFilter),
    Status(AgentStatusFilter),
    Revision(AgentRevisionFilter),
    CreatedAt(AgentCreatedAtFilter),
    Env(AgentEnvFilter),
    And(AgentAndFilter),
    Or(AgentOrFilter),
    Not(AgentNotFilter),
    Config(AgentConfigVarsFilter),
}

impl AgentFilter {
    pub fn and(&self, filter: AgentFilter) -> Self {
        match self.clone() {
            AgentFilter::And(AgentAndFilter { filters }) => {
                Self::new_and([filters, vec![filter]].concat())
            }
            f => Self::new_and(vec![f, filter]),
        }
    }

    pub fn or(&self, filter: AgentFilter) -> Self {
        match self.clone() {
            AgentFilter::Or(AgentOrFilter { filters }) => {
                Self::new_or([filters, vec![filter]].concat())
            }
            f => Self::new_or(vec![f, filter]),
        }
    }

    pub fn not(&self) -> Self {
        Self::new_not(self.clone())
    }

    pub fn new_and(filters: Vec<AgentFilter>) -> Self {
        AgentFilter::And(AgentAndFilter::new(filters))
    }

    pub fn new_or(filters: Vec<AgentFilter>) -> Self {
        AgentFilter::Or(AgentOrFilter::new(filters))
    }

    pub fn new_not(filter: AgentFilter) -> Self {
        AgentFilter::Not(AgentNotFilter::new(filter))
    }

    pub fn new_name(comparator: StringFilterComparator, value: String) -> Self {
        AgentFilter::Name(AgentNameFilter::new(comparator, value))
    }

    pub fn new_env(name: String, comparator: StringFilterComparator, value: String) -> Self {
        AgentFilter::Env(AgentEnvFilter::new(name, comparator, value))
    }

    pub fn new_config(
        name: String,
        comparator: StringFilterComparator,
        value: String,
    ) -> Self {
        AgentFilter::Config(AgentConfigVarsFilter::new(name, comparator, value))
    }

    pub fn new_revision(comparator: FilterComparator, value: ComponentRevision) -> Self {
        AgentFilter::Revision(AgentRevisionFilter::new(comparator, value))
    }

    pub fn new_status(comparator: FilterComparator, value: AgentStatus) -> Self {
        AgentFilter::Status(AgentStatusFilter::new(comparator, value))
    }

    pub fn new_created_at(comparator: FilterComparator, value: Timestamp) -> Self {
        AgentFilter::CreatedAt(AgentCreatedAtFilter::new(comparator, value))
    }

    pub fn from(filters: Vec<String>) -> Result<AgentFilter, String> {
        let mut fs = Vec::new();
        for f in filters {
            fs.push(AgentFilter::from_str(&f)?);
        }
        Ok(AgentFilter::new_and(fs))
    }
}

impl Display for AgentFilter {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentFilter::Name(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Revision(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Status(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::CreatedAt(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Env(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Config(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Not(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::And(filter) => {
                write!(f, "{filter}")
            }
            AgentFilter::Or(filter) => {
                write!(f, "{filter}")
            }
        }
    }
}

impl FromStr for AgentFilter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let elements = s.split_whitespace().collect::<Vec<&str>>();

        if elements.len() == 3 {
            let arg = elements[0];
            let comparator = elements[1];
            let value = elements[2];
            match arg {
                "name" => Ok(AgentFilter::new_name(
                    comparator.parse()?,
                    value.to_string(),
                )),
                "revision" => Ok(AgentFilter::new_revision(
                    comparator.parse()?,
                    value
                        .parse()
                        .map_err(|e| format!("Invalid filter value: {e}"))?,
                )),
                "status" => Ok(AgentFilter::new_status(comparator.parse()?, value.parse()?)),
                "created_at" | "createdAt" => Ok(AgentFilter::new_created_at(
                    comparator.parse()?,
                    value.parse()?,
                )),
                _ if arg.starts_with("env.") => {
                    let name = &arg[4..];
                    Ok(AgentFilter::new_env(
                        name.to_string(),
                        comparator.parse()?,
                        value.to_string(),
                    ))
                }
                _ if arg.starts_with("config.") => {
                    let name = &arg[7..];
                    Ok(AgentFilter::new_config(
                        name.to_string(),
                        comparator.parse()?,
                        value.to_string(),
                    ))
                }
                _ => Err(format!("Invalid filter: {s}")),
            }
        } else {
            Err(format!("Invalid filter: {s}"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum StringFilterComparator {
    Equal,
    NotEqual,
    Like,
    NotLike,
    StartsWith,
}

impl StringFilterComparator {
    pub fn matches<T: Display>(&self, value1: &T, value2: &T) -> bool {
        match self {
            StringFilterComparator::Equal => value1.to_string() == value2.to_string(),
            StringFilterComparator::NotEqual => value1.to_string() != value2.to_string(),
            StringFilterComparator::Like => {
                value1.to_string().contains(value2.to_string().as_str())
            }
            StringFilterComparator::NotLike => {
                !value1.to_string().contains(value2.to_string().as_str())
            }
            StringFilterComparator::StartsWith => {
                value1.to_string().starts_with(value2.to_string().as_str())
            }
        }
    }
}

impl FromStr for StringFilterComparator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(StringFilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(StringFilterComparator::NotEqual),
            "like" => Ok(StringFilterComparator::Like),
            "notlike" => Ok(StringFilterComparator::NotLike),
            "startswith" => Ok(StringFilterComparator::StartsWith),
            _ => Err(format!("Unknown String Filter Comparator: {s}")),
        }
    }
}

impl TryFrom<i32> for StringFilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(StringFilterComparator::Equal),
            1 => Ok(StringFilterComparator::NotEqual),
            2 => Ok(StringFilterComparator::Like),
            3 => Ok(StringFilterComparator::NotLike),
            4 => Ok(StringFilterComparator::StartsWith),
            _ => Err(format!("Unknown String Filter Comparator: {value}")),
        }
    }
}

impl From<StringFilterComparator> for i32 {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => 0,
            StringFilterComparator::NotEqual => 1,
            StringFilterComparator::Like => 2,
            StringFilterComparator::NotLike => 3,
            StringFilterComparator::StartsWith => 4,
        }
    }
}

impl Display for StringFilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StringFilterComparator::Equal => "==",
            StringFilterComparator::NotEqual => "!=",
            StringFilterComparator::Like => "like",
            StringFilterComparator::NotLike => "notlike",
            StringFilterComparator::StartsWith => "startswith",
        };
        write!(f, "{s}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum FilterComparator {
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
    LessEqual,
    Less,
}

impl Display for FilterComparator {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FilterComparator::Equal => "==",
            FilterComparator::NotEqual => "!=",
            FilterComparator::GreaterEqual => ">=",
            FilterComparator::Greater => ">",
            FilterComparator::LessEqual => "<=",
            FilterComparator::Less => "<",
        };
        write!(f, "{s}")
    }
}

impl FilterComparator {
    pub fn matches<T: Ord>(&self, value1: &T, value2: &T) -> bool {
        match self {
            FilterComparator::Equal => value1 == value2,
            FilterComparator::NotEqual => value1 != value2,
            FilterComparator::Less => value1 < value2,
            FilterComparator::LessEqual => value1 <= value2,
            FilterComparator::Greater => value1 > value2,
            FilterComparator::GreaterEqual => value1 >= value2,
        }
    }
}

impl FromStr for FilterComparator {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "==" | "=" | "equal" | "eq" => Ok(FilterComparator::Equal),
            "!=" | "notequal" | "ne" => Ok(FilterComparator::NotEqual),
            ">=" | "greaterequal" | "ge" => Ok(FilterComparator::GreaterEqual),
            ">" | "greater" | "gt" => Ok(FilterComparator::Greater),
            "<=" | "lessequal" | "le" => Ok(FilterComparator::LessEqual),
            "<" | "less" | "lt" => Ok(FilterComparator::Less),
            _ => Err(format!("Unknown Filter Comparator: {s}")),
        }
    }
}

impl TryFrom<i32> for FilterComparator {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(FilterComparator::Equal),
            1 => Ok(FilterComparator::NotEqual),
            2 => Ok(FilterComparator::Less),
            3 => Ok(FilterComparator::LessEqual),
            4 => Ok(FilterComparator::Greater),
            5 => Ok(FilterComparator::GreaterEqual),
            _ => Err(format!("Unknown Filter Comparator: {value}")),
        }
    }
}

impl From<FilterComparator> for i32 {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => 0,
            FilterComparator::NotEqual => 1,
            FilterComparator::Less => 2,
            FilterComparator::LessEqual => 3,
            FilterComparator::Greater => 4,
            FilterComparator::GreaterEqual => 5,
        }
    }
}
