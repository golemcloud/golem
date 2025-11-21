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

use super::path_pattern_parser::parse_path_pattern;
use desert_rust::BinaryCodec;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec)]
pub struct AllPathPatterns {
    pub path_patterns: Vec<PathPattern>,
    pub query_params: Vec<QueryInfo>,
}

impl AllPathPatterns {
    pub fn parse(input: &str) -> Result<AllPathPatterns, String> {
        input.parse()
    }
}

impl Display for AllPathPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for pattern in self.path_patterns.iter() {
            write!(f, "/")?;
            write!(f, "{pattern}")?;
        }

        if !self.query_params.is_empty() {
            write!(f, "?")?;
            for (index, query) in self.query_params.iter().enumerate() {
                if index > 0 {
                    write!(f, "&")?;
                }
                write!(f, "{query}")?;
            }
        }

        Ok(())
    }
}

impl FromStr for AllPathPatterns {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_path_pattern(s)
            .map_err(|err| err.to_string())
            .and_then(|(leftover, result)| {
                if !leftover.is_empty() {
                    Err("Failed to parse path".to_string())
                } else {
                    Ok(result)
                }
            })
    }
}

impl<'de> Deserialize<'de> for AllPathPatterns {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value {
            Value::String(value) => match AllPathPatterns::parse(value.as_str()) {
                Ok(path_pattern) => Ok(path_pattern),
                Err(message) => Err(serde::de::Error::custom(message.to_string())),
            },

            _ => Err(serde::de::Error::custom("Failed to parse path from yaml")),
        }
    }
}

impl Serialize for AllPathPatterns {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = Value::String(self.to_string());
        Value::serialize(&value, serializer)
    }
}

/// Invariant: PathPattern::CatchAllVar is only allowed at the end of the path
#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum PathPattern {
    Literal(LiteralInfo),
    Var(VarInfo),
    CatchAllVar(VarInfo),
}

impl PathPattern {
    pub fn literal(value: impl Into<String>) -> PathPattern {
        PathPattern::Literal(LiteralInfo(value.into()))
    }

    pub fn var(value: impl Into<String>) -> PathPattern {
        PathPattern::Var(VarInfo {
            key_name: value.into(),
        })
    }

    pub fn catch_all_var(value: impl Into<String>) -> PathPattern {
        PathPattern::CatchAllVar(VarInfo {
            key_name: value.into(),
        })
    }
}

impl Display for PathPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathPattern::Literal(info) => write!(f, "{}", info.0),
            PathPattern::Var(info) => write!(f, "{{{}}}", info.key_name),
            PathPattern::CatchAllVar(info) => write!(f, "{{+{}}}", info.key_name),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec)]
#[desert(transparent)]
pub struct LiteralInfo(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub struct VarInfo {
    pub key_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub struct QueryInfo {
    pub key_name: String,
}

impl Display for QueryInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{{}}}", self.key_name)
    }
}
