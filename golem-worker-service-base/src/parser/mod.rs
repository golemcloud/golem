use std::fmt;

pub mod expr_parser;
pub mod literal_parser;
pub mod path_pattern_parser;
pub mod place_holder_parser;

pub trait GolemParser<T> {
    fn parse(&self, str: &str) -> Result<T, ParseError>;
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    Message(String),
}

impl From<&str> for ParseError {
    fn from(s: &str) -> Self {
        ParseError::Message(s.to_string())
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Message(msg) => write!(f, "{}", msg),
        }
    }
}
