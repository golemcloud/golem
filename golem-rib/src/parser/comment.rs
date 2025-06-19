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

use combine::parser::char::{char, spaces, string};
use combine::{
    any, attempt, choice, eof, many, none_of, not_followed_by, optional, parser, Parser, Stream,
};

fn comments_<Input>() -> impl Parser<Input, Output = Option<()>>
where
    Input: Stream<Token = char>,
{
    spaces()
        .silent()
        .with(
            line_or_block_comments()
                .silent()
                .skip(spaces().silent())
                .map(|_| ()),
        )
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
        spaces().with(choice!(attempt(string("/**")), attempt(string("/*"))).map(|_| ())),
        many(attempt(choice((
            (char('*'), not_followed_by(char('/'))).map(|_| ()),
            any().map(|_| ()),
        ))))
        .map(|_: Vec<_>| ()),
        string("*/").map(|_| ()),
        optional(comments()).map(|_: Option<_>| ()),
    )
        .map(|(_, _, _, _): ((), (), (), ())| ())
}
fn line_comment<Input>() -> impl Parser<Input, Output = ()>
where
    Input: Stream<Token = char>,
{
    (
        attempt(string("//").map(|_| ())),
        many(none_of(vec!['\n']).map(|_| ())),
        choice((string("\r\n").map(|_| ()), string("\n").map(|_| ()), eof())),
        optional(comments()).map(|_: Option<_>| ()),
    )
        .map(|(_, _, _, _): ((), (), (), ())| ())
}

parser! {
    pub fn comments[Input]()(Input) -> Option<()>
    where [Input: Stream<Token = char>,]
    {
        comments_()
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::comment::comments;
    use combine::EasyParser;
    use test_r::test;

    #[test]
    fn test_parse_line_comment_1() {
        let input = r#"
        // This is a line comment


        "#;
        let (_, remaining) = comments().easy_parse(input).unwrap();
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_line_comment_2() {
        let input = r#"
        // This is a line comment
        foo"#;
        let (_, remaining) = comments().easy_parse(input).unwrap();
        assert_eq!(remaining, "foo");
    }

    #[test]
    fn test_parse_line_comment_3() {
        let input = r#"
        // This is a line comment
        // foo"#;
        let (_, remaining) = comments().easy_parse(input).unwrap();
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_parse_mixed_comments_4() {
        let input = r#"
        // This is a line / comment with `code`
        "#;
        let (_, remaining) = comments().easy_parse(input).unwrap();
        assert_eq!(remaining, "");
    }
}
