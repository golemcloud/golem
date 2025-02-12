use crate::parser::errors::RibParseError;
use crate::type_parameter::TypeParameter;
use combine::error::StreamError;
use combine::parser::char::{alpha_num, char, spaces, string};
use combine::stream::Stream;
use combine::{attempt, choice, many1, not_followed_by, optional, parser, sep_by, ParseError, Parser};

// Parser for TypeParameter
pub fn type_parameter<Input>() -> impl Parser<Input, Output = TypeParameter>
where
    Input: Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
{
    choice!(
        attempt(fully_qualified_interface_name().map(TypeParameter::FullyQualifiedInterface)),
        attempt(package_name().map(TypeParameter::PackageName)),
        interface_name().map(TypeParameter::Interface),
    )
}

mod internal {
    use crate::parser::errors::RibParseError;
    use crate::type_parameter::{FullyQualifiedInterfaceName, InterfaceName, PackageName};
    use combine::parser::char::{alpha_num, char};
    use combine::stream::Stream;
    use combine::{many1, optional, ParseError, Parser};

    fn fully_qualified_interface_name<Input>() -> impl Parser<Input, Output = FullyQualifiedInterfaceName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
    {
        (package_name().skip(char('/')), interface_name())
            .map(|(package_name, interface_name)| FullyQualifiedInterfaceName {
                package_name,
                interface_name,
            })
    }


    fn package_name<Input>() -> impl Parser<Input, Output = PackageName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
    {
        let namespace = many1(alpha_num().or(char('-')).or(char('_')));
        let package_name = many1(alpha_num().or(char('-')).or(char('_')));
        let version = optional(char('@').with(many1(alpha_num().or(char('.')).or(char('-')))));

        (namespace.skip(char(':')), package_name, version)
            .map(|(namespace, package_name, version)| PackageName {
                namespace,
                package_name,
                version: version.map(|v| v.iter().collect()),
            })
    }

    fn interface_name<Input>() -> impl Parser<Input, Output = InterfaceName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,
    {
        let name = many1(alpha_num().or(char('-')).or(char('_')));
        let version = optional(char('@').with(many1(alpha_num().or(char('.')).or(char('-')))));

        (name, version)
            .map(|(name, version)| InterfaceName {
                name,
                version: version.map(|v| v.iter().collect()),
            })
    }
}