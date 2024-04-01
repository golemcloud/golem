use nom::bytes::complete::{tag, take_while};
use nom::IResult;
use nom::sequence::delimited;

use super::*;

pub struct PlaceHolderPatternParser;

pub struct PlaceHolder(pub String);

impl GolemParser<PlaceHolder> for PlaceHolderPatternParser {
    fn parse(&self, str: &str) -> Result<PlaceHolder, ParseError> {
        match parse_place_holder(str) {
            Ok(value) => Ok(value.1),
            Err(err) => Result::Err(ParseError::Message(err.to_string())),
        }
    }
}

pub fn parse_place_holder(input: &str) -> IResult<&str, PlaceHolder> {
    delimited(tag("{"), take_while(|c| c != '}'), tag("}"))(input)
        .map(|(rest, captured)| (rest, PlaceHolder(captured.to_string())))
}
