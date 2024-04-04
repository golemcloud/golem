use std::fmt;

pub(crate) mod expr_parser;
pub(crate) mod path_pattern_parser;
pub(crate) mod place_holder_parser;

pub trait GolemParser<T> {
    fn parse(&self, str: &str) -> Result<T, ParseError>;
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    Message(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ParseError::Message(msg) => write!(f, "{}", msg),
        }
    }
}
