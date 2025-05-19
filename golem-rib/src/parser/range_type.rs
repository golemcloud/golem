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

use combine::parser::char::{spaces, string};
use combine::{optional, ParseError, Parser, Stream};

use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;

// This is range avoiding left recursion
#[derive(Clone, Debug)]
pub enum RangeType {
    Inclusive,
    Exclusive,
}
pub fn range_type<Input>() -> impl Parser<Input, Output = RangeType>
where
    Input: Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    (string(".."), optional(string("=").skip(spaces()))).map(|(_, d): (_, Option<_>)| match d {
        Some(_) => RangeType::Inclusive,
        None => RangeType::Exclusive,
    })
}
