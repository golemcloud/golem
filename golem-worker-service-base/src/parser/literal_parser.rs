use nom::character::complete::not_line_ending;
use nom::combinator::all_consuming;
use nom::IResult;
use crate::http::http_api_definition::LiteralInfo;

use super::*;

pub struct LiteralParser;

impl GolemParser<LiteralInfo> for LiteralParser {
    fn parse(&self, str: &str) -> Result<LiteralInfo, ParseError> {
        match parse_literal_pattern(str) {
            Ok(value) => Ok(value.1),
            Err(err) => Result::Err(ParseError::Message(err.to_string())),
        }
    }
}

pub fn parse_literal_pattern(input: &str) -> IResult<&str, LiteralInfo> {
    all_consuming(not_line_ending)(input)
        .map(|(rest, captured)| (rest, LiteralInfo(captured.to_string())))
}
