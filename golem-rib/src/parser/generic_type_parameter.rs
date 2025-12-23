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

use crate::generic_type_parameter::GenericTypeParameter;
use crate::parser::RibParseError;
use crate::rib_source_span::GetSourcePosition;
use combine::parser::char::{alpha_num, char as char_};
use combine::{many1, ParseError, Parser};

pub fn generic_type_parameter<Input>() -> impl Parser<Input, Output = GenericTypeParameter>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
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
