use std::fmt;

pub(crate) mod path_pattern_parser;
pub(crate) mod place_holder_parser;

pub trait GolemParser<T> {
    fn parse(&self, str: &str) -> Result<T, ParseError>;
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    Message(String),
}

impl<T: AsRef<str>> From<T> for ParseError {
    fn from(msg: T) -> Self {
        ParseError::Message(msg.as_ref().to_string())
    }
}

impl ParseError {
    pub fn message(msg: impl Into<String>) -> Self {
        ParseError::Message(msg.into())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Message(msg) => write!(f, "{}", msg),
        }
    }
}
