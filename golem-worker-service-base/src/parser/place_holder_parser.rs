use nom::bytes::complete::tag;
use nom::sequence::delimited;
use nom::IResult;

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
    delimited(tag("{"), take_until_unbalanced('{', '}'), tag("}"))(input)
        .map(|(rest, captured)| (rest, PlaceHolder(captured.to_string())))
}

// https://stackoverflow.com/questions/70630556/parse-allowing-nested-parentheses-in-nom
fn take_until_unbalanced(
    opening_bracket: char,
    closing_bracket: char,
) -> impl Fn(&str) -> IResult<&str, &str> {
    move |i: &str| {
        let mut index = 0;
        let mut bracket_counter = 0;
        while let Some(n) = &i[index..].find(&[opening_bracket, closing_bracket, '\\'][..]) {
            index += n;
            let mut it = i[index..].chars();
            match it.next().unwrap_or_default() {
                '\\' => {
                    // Skip the escape char `\`.
                    index += '\\'.len_utf8();
                    // Skip also the following char.
                    let c = it.next().unwrap_or_default();
                    index += c.len_utf8();
                }
                c if c == opening_bracket => {
                    bracket_counter += 1;
                    index += opening_bracket.len_utf8();
                }
                c if c == closing_bracket => {
                    // Closing bracket.
                    bracket_counter -= 1;
                    index += closing_bracket.len_utf8();
                }
                // Can not happen.
                _ => unreachable!(),
            };
            // We found the unmatched closing bracket.
            if bracket_counter == -1 {
                // We do not consume it.
                index -= closing_bracket.len_utf8();
                return Ok((&i[index..], &i[0..index]));
            };
        }

        if bracket_counter == 0 {
            Ok(("", i))
        } else {
            let error = nom::error::Error::new(i, nom::error::ErrorKind::TakeUntil);
            let error = nom::Err::Error(error);
            Err(error)
        }
    }
}
