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

use combine::parser::char::{alpha_num, char, newline, spaces, string};
use combine::{
    any, attempt, choice, eof, many, none_of, not_followed_by, optional, Parser, Stream,
};

pub fn comments<Input>() -> impl Parser<Input, Output = Option<()>>
where
    Input: Stream<Token = char>,
{
    spaces()
        .silent()
        .with(line_or_block_comments().silent().skip(spaces().silent()))
        .map(|_| None)
}

fn line_or_block_comments<Input>() -> impl Parser<Input, Output = Option<()>>
where
    Input: Stream<Token = char>,
{
    optional(choice!(line_comment(), block_comment()))
}

fn block_comment<Input>() -> impl Parser<Input, Output = ()>
where
    Input: Stream<Token = char>,
{
    (
        choice!(attempt(string("/**")), attempt(string("/*"))).map(|_| ()),
        many(
            attempt(
                choice((
                    (char('*'), not_followed_by(char('/'))).map(|_| ()),
                    any().map(|_| ()),
                ))
            )
        )
        .map(|_: Vec<_>| ()),
        string("*/").map(|_| ()),
    )
        .map(|(_, _, _): ((), (), ())| ())
}
fn line_comment<Input>() -> impl Parser<Input, Output = ()>
where
    Input: Stream<Token = char>,
{
    (
        attempt(string("//").map(|_| ())),
        many(none_of(vec!['\n']).map(|_| ())),
        choice((string("\r\n").map(|_| ()), string("\n").map(|_| ()), eof())),
    )
        .map(|(_, _, _): ((), (), ())| ())
}
