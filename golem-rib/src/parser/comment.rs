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

use combine::{attempt, choice, eof, many, none_of, optional, Parser, Stream};
use combine::parser::char::{char, spaces, string};

pub fn comment<Input>() -> impl Parser<Input, Output = Option<()>>
where
    Input: Stream<Token = char>,
{
  optional(comment_1())
}


pub fn comment_1<Input>() -> impl Parser<Input, Output = ()>
where
    Input: Stream<Token = char>,
{
    (
        attempt(string("//")).map(|_| ()),
        many(
            none_of(vec!['\n']).map(|_| ()),
        ),
        choice((
            string("\r\n").map(|_| ()),
            string("\n").map(|_| ()),
            eof()
        )),
    )
        .map(|(_, _, _): ((), (), ())| ())
}

pub fn discard<Input>() -> impl Parser<Input, Output = Option<()>>
where
    Input: Stream<Token = char>,
{
    spaces().silent()
        .with(comment().silent().skip(spaces().silent()))
        .map(|_| None)
}
