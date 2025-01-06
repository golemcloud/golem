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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterPattern {
    Literal(LiteralPattern),
    Variable,
    CatchAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LiteralPattern(pub String);

impl RouterPattern {
    pub fn literal(literal: impl Into<String>) -> Self {
        Self::Literal(LiteralPattern(literal.into()))
    }

    #[inline]
    pub fn parse(s: impl AsRef<str>) -> Vec<RouterPattern> {
        Self::split(s.as_ref()).map(Self::parse_single).collect()
    }

    #[inline]
    pub fn split(s: &str) -> impl Iterator<Item = &str> {
        s.trim_matches('/').split('/')
    }

    #[inline]
    fn parse_single(s: &str) -> RouterPattern {
        if s.starts_with(':') || (s.starts_with('{') && s.ends_with('}')) {
            RouterPattern::Variable
        } else if s == "*" {
            RouterPattern::CatchAll
        } else {
            RouterPattern::literal(s)
        }
    }
}

use crate::gateway_api_definition::http::PathPattern;

impl From<PathPattern> for RouterPattern {
    fn from(path: PathPattern) -> Self {
        match path {
            PathPattern::Literal(literal) => RouterPattern::literal(literal.0),
            PathPattern::Var(_) => RouterPattern::Variable,
            PathPattern::CatchAllVar(_) => RouterPattern::CatchAll,
        }
    }
}

// Needed to get value in map by &str
impl std::borrow::Borrow<str> for LiteralPattern {
    fn borrow(&self) -> &str {
        &self.0
    }
}
