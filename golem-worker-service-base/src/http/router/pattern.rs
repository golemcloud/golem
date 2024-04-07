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

    pub fn parse(s: impl AsRef<str>) -> Vec<RouterPattern> {
        Self::split(s.as_ref()).map(Self::parse_single).collect()
    }

    pub fn split(s: &str) -> impl Iterator<Item = &str> {
        s.trim_matches('/').split('/')
    }

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

use crate::api_definition::http::PathPattern;

impl From<PathPattern> for RouterPattern {
    fn from(path: PathPattern) -> Self {
        match path {
            PathPattern::Literal(literal) => RouterPattern::literal(literal.0),
            PathPattern::Var(_) => RouterPattern::Variable,
        }
    }
}

// Needed to get value in map by &str
impl std::borrow::Borrow<str> for LiteralPattern {
    fn borrow(&self) -> &str {
        &self.0
    }
}
