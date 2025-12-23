// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::parser::RibParseError;
use crate::rib_source_span::GetSourcePosition;
use crate::type_parameter::TypeParameter;
use combine::stream::Stream;
use combine::{attempt, choice, ParseError, Parser};
use internal::*;

// Parser for TypeParameter
pub fn type_parameter<Input>() -> impl Parser<Input, Output = TypeParameter>
where
    Input: Stream<Token = char>,
    Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    choice((
        attempt(fully_qualified_interface_name().map(TypeParameter::FullyQualifiedInterface)),
        attempt(package_name().map(TypeParameter::PackageName)),
        attempt(interface_name().map(TypeParameter::Interface)),
    ))
}

mod internal {
    use crate::parser::RibParseError;
    use crate::rib_source_span::GetSourcePosition;
    use crate::type_parameter::{FullyQualifiedInterfaceName, InterfaceName, PackageName};
    use combine::parser::char::{alpha_num, char as char_};
    use combine::stream::Stream;
    use combine::{many1, optional, ParseError, Parser};

    pub(crate) fn fully_qualified_interface_name<Input>(
    ) -> impl Parser<Input, Output = FullyQualifiedInterfaceName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        (package_name().skip(char_('/')), interface_name()).map(|(package_name, interface_name)| {
            FullyQualifiedInterfaceName {
                package_name,
                interface_name,
            }
        })
    }

    pub(crate) fn package_name<Input>() -> impl Parser<Input, Output = PackageName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        let namespace = many1(alpha_num().or(char_('-')).or(char_('_')));
        let package_name = many1(alpha_num().or(char_('-')).or(char_('_')));
        let version = optional(char_('@').with(version()));

        (namespace.skip(char_(':')), package_name, version).map(
            |(namespace, package_name, version)| PackageName {
                namespace,
                package_name,
                version,
            },
        )
    }

    pub(crate) fn interface_name<Input>() -> impl Parser<Input, Output = InterfaceName>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        let name = many1(alpha_num().or(char_('-')).or(char_('_')));
        let version = optional(char_('@').with(version()));

        (name, version).map(|(name, version)| InterfaceName { name, version })
    }

    fn version<Input>() -> impl Parser<Input, Output = String>
    where
        Input: Stream<Token = char>,
        Input::Error: ParseError<Input::Token, Input::Range, Input::Position>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        many1(alpha_num().or(char_('.')).or(char_('-'))).map(|s: Vec<char>| s.into_iter().collect())
    }
}
