use combine::{many1, ParseError, Parser};
use combine::parser::char::{alpha_num, char as char_};
use crate::generic_type_parameter::GenericTypeParameter;
use crate::parser::RibParseError;

pub fn generic_type_parameter<Input>() -> impl Parser<Input, Output = GenericTypeParameter>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    many1(
        alpha_num() // Alphanumeric characters
            .or(char_('.')) // Period
            .or(char_('-')) // Hyphen
            .or(char_('@')) // At symbol
            .or(char_(':')) // Colon
            .or(char_('/')),
    )
        .map(|chars: Vec<char>| GenericTypeParameter {
            value: chars.into_iter().collect(),
        })
}