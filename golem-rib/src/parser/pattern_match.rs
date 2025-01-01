// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use combine::parser::char::{alpha_num, char, spaces, string};
use combine::{attempt, not_followed_by, sep_by1, ParseError, Parser};

use match_arm::*;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::rib_expr::rib_expr;

pub fn pattern_match<Input>() -> impl Parser<Input, Output = Expr>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
{
    let arms = sep_by1(match_arm().skip(spaces()), char(',').skip(spaces()));

    attempt(
        string("match")
            .skip(not_followed_by(alpha_num().or(char('_')).or(char('-'))))
            .skip(spaces()),
    )
    .with(
        (
            rib_expr().skip(spaces()),
            char('{').skip(spaces()),
            arms.skip(spaces()),
            char('}').skip(spaces()),
        )
            .map(|(expr, _, arms, _)| Expr::pattern_match(expr, arms)),
    )
    .message("Invalid syntax for pattern match")
}

mod match_arm {
    use combine::parser::char::spaces;
    use combine::{parser::char::string, ParseError, Parser};

    use crate::expr::MatchArm;
    use crate::parser::errors::RibParseError;
    use crate::parser::rib_expr::rib_expr;

    use super::arm_pattern::*;

    // RHS of a match arm
    pub(crate) fn match_arm<Input>() -> impl Parser<Input, Output = MatchArm>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        (
            //LHS
            arm_pattern().skip(spaces()),
            string("=>").skip(spaces()),
            //RHS
            rib_expr().skip(spaces()),
        )
            .map(|(lhs, _, rhs)| MatchArm::new(lhs, rhs))
    }
}

// Keep the module structure same to avoid recursion related compiler errors
mod arm_pattern {
    use combine::attempt;
    use combine::parser::char::spaces;
    use combine::{choice, parser, parser::char::char, ParseError, Parser, Stream};

    use crate::expr::ArmPattern;
    use crate::parser::errors::RibParseError;
    use crate::parser::pattern_match::internal::*;

    // LHS of a match arm
    fn arm_pattern_<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((
            attempt(arm_pattern_constructor()),
            attempt(char('_').map(|_| ArmPattern::WildCard)),
            attempt(
                (
                    alias_name().skip(spaces()),
                    char('@').skip(spaces()),
                    arm_pattern().skip(spaces()),
                )
                    .map(|(iden, _, pattern)| ArmPattern::As(iden, Box::new(pattern))),
            ),
            attempt(arm_pattern_literal()),
        ))
    }

    parser! {
        pub(crate) fn arm_pattern[Input]()(Input) -> ArmPattern
         where [Input: Stream<Token = char>, RibParseError: Into<<Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError>,]{
            arm_pattern_()
        }
    }
}

mod internal {
    use combine::many1;
    use combine::parser::char::{digit, letter};
    use combine::parser::char::{spaces, string};
    use combine::sep_by;
    use combine::{attempt, sep_by1};
    use combine::{choice, ParseError};
    use combine::{parser::char::char as char_, Parser};

    use crate::expr::ArmPattern;
    use crate::parser::errors::RibParseError;
    use crate::parser::pattern_match::arm_pattern::*;

    use crate::parser::rib_expr::rib_expr;

    pub(crate) fn arm_pattern_constructor<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        choice((
            attempt(arm_pattern_constructor_with_name()),
            attempt(tuple_arm_pattern_constructor()),
            attempt(list_arm_pattern_constructor()),
            attempt(record_arm_pattern_constructor()),
        ))
    }

    pub(crate) fn arm_pattern_literal<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        rib_expr().map(|lit| ArmPattern::Literal(Box::new(lit)))
    }

    pub(crate) fn alias_name<Input>() -> impl Parser<Input, Output = String>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many1(letter().or(digit()).or(char_('_')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse alias name")
    }

    fn arm_pattern_constructor_with_name<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        let custom = (
            constructor_type_name().skip(spaces()),
            string("(").skip(spaces()),
            sep_by(arm_pattern().skip(spaces()), char_(',').skip(spaces())),
            string(")").skip(spaces()),
        )
            .map(|(name, _, patterns, _)| ArmPattern::Constructor(name, patterns));

        attempt(none_constructor()).or(custom)
    }

    fn none_constructor<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        string("none").map(|_| ArmPattern::constructor("none", vec![]))
    }

    fn tuple_arm_pattern_constructor<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        (
            string("(").skip(spaces()),
            sep_by(arm_pattern().skip(spaces()), char_(',').skip(spaces())),
            string(")").skip(spaces()),
        )
            .map(|(_, patterns, _)| ArmPattern::TupleConstructor(patterns))
    }

    fn list_arm_pattern_constructor<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        (
            string("[").skip(spaces()),
            sep_by(arm_pattern().skip(spaces()), char_(',').skip(spaces())),
            string("]").skip(spaces()),
        )
            .map(|(_, patterns, _)| ArmPattern::ListConstructor(patterns))
    }

    struct KeyArmPattern {
        key: String,
        pattern: ArmPattern,
    }

    fn record_arm_pattern_constructor<Input>() -> impl Parser<Input, Output = ArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        (
            string("{").skip(spaces()),
            sep_by1(key_arm_pattern().skip(spaces()), char_(',').skip(spaces())),
            string("}").skip(spaces()),
        )
            .map(|(_, patterns, _)| {
                let patterns: Vec<KeyArmPattern> = patterns;
                ArmPattern::RecordConstructor(
                    patterns
                        .into_iter()
                        .map(|pattern| (pattern.key, pattern.pattern))
                        .collect(),
                )
            })
    }

    fn key_arm_pattern<Input>() -> impl Parser<Input, Output = KeyArmPattern>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        (
            record_key().skip(spaces()),
            char_(':').skip(spaces()),
            arm_pattern(),
        )
            .map(|(var, _, arm_pattern)| KeyArmPattern {
                key: var,
                pattern: arm_pattern,
            })
    }

    fn record_key<Input>() -> impl Parser<Input, Output = String>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many1(letter().or(char_('_').or(char_('-'))))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Invalid identifier")
    }

    fn constructor_type_name<Input>() -> impl Parser<Input, Output = String>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
    {
        many1(letter().or(digit()).or(char_('_')).or(char_('-')))
            .map(|s: Vec<char>| s.into_iter().collect())
            .message("Unable to parse custom constructor name")
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use combine::stream::position;
    use combine::EasyParser;

    use crate::expr::ArmPattern;
    use crate::expr::Expr;
    use crate::expr::MatchArm;

    use super::*;

    #[test]
    fn test_simple_pattern_match() {
        let input = "match foo { _ => bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::pattern_match(
                    Expr::identifier("foo"),
                    vec![MatchArm::new(ArmPattern::WildCard, Expr::identifier("bar"))]
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_simple_pattern_with_wild_card() {
        let input = "match foo { foo(_, _, iden)  => bar }";
        let result = rib_expr()
            .easy_parse(position::Stream::new(input))
            .map(|x| x.0);
        assert_eq!(
            result,
            Ok(Expr::pattern_match(
                Expr::identifier("foo"),
                vec![MatchArm::new(
                    ArmPattern::custom_constructor(
                        "foo",
                        vec![
                            ArmPattern::WildCard,
                            ArmPattern::WildCard,
                            ArmPattern::identifier("iden")
                        ]
                    ),
                    Expr::identifier("bar")
                )]
            ))
        );
    }

    #[test]
    fn test_simple_pattern_with_alias() {
        let input = "match foo { abc @ foo(_, _, d @ baz(_)) => bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::pattern_match(
                    Expr::identifier("foo"),
                    vec![MatchArm::new(
                        ArmPattern::As(
                            "abc".to_string(),
                            Box::new(ArmPattern::custom_constructor(
                                "foo",
                                vec![
                                    ArmPattern::WildCard,
                                    ArmPattern::WildCard,
                                    ArmPattern::As(
                                        "d".to_string(),
                                        Box::new(ArmPattern::custom_constructor(
                                            "baz",
                                            vec![ArmPattern::WildCard]
                                        ))
                                    )
                                ]
                            ))
                        ),
                        Expr::identifier("bar")
                    )]
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_pattern_match_with_custom_constructor() {
        let input = "match foo { Foo(x) => bar }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::pattern_match(
                    Expr::identifier("foo"),
                    vec![MatchArm::new(
                        ArmPattern::Constructor(
                            "Foo".to_string(),
                            vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))]
                        ),
                        Expr::identifier("bar")
                    )]
                ),
                ""
            ))
        );
    }

    #[test]
    fn test_pattern_match() {
        let input = "match foo { _ => bar, ok(x) => x, err(x) => x, none => foo, some(x) => x }";
        let result = rib_expr().easy_parse(input);
        assert_eq!(
            result,
            Ok((
                Expr::pattern_match(
                    Expr::identifier("foo"),
                    vec![
                        MatchArm::new(ArmPattern::WildCard, Expr::identifier("bar")),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "ok",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))],
                            ),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "err",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))],
                            ),
                            Expr::identifier("x"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor("none", vec![]),
                            Expr::identifier("foo"),
                        ),
                        MatchArm::new(
                            ArmPattern::constructor(
                                "some",
                                vec![ArmPattern::Literal(Box::new(Expr::identifier("x")))],
                            ),
                            Expr::identifier("x"),
                        ),
                    ]
                ),
                ""
            ))
        );
    }
}
