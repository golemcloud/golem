use crate::generic_type_parameter::GenericTypeParameter;
use crate::parser::RibParseError;
use combine::parser::char::{alpha_num, char as char_};
use combine::{many1, ParseError, Parser};
use crate::rib_source_span::GetSourcePosition;

pub fn generic_type_parameter<Input>() -> impl Parser<Input, Output = GenericTypeParameter>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition
{
    many1(
        alpha_num()
            .or(char_('.'))
            .or(char_('-'))
            .or(char_('@'))
            .or(char_(':'))
            .or(char_('/')),
    )
    .map(|chars: Vec<char>| GenericTypeParameter {
        value: chars.into_iter().collect(),
    })
}
