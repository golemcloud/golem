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

use bigdecimal::ToPrimitive;
use combine::parser::char::{char as char_, spaces};
use combine::{attempt, choice, many1, optional, ParseError, Parser};

use internal::*;

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier;
use crate::Range;
use crate::rib_source_span::GetSourcePosition;

pub fn range<Input>() -> impl Parser<Input, Output = Range>
where
    Input: combine::Stream<Token = char>,
    RibParseError: Into<
        <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
    >,
    Input::Position: GetSourcePosition,
{
    spaces().with(internal::range().skip(spaces()).map(|range| range))
}

mod internal {
    use bigdecimal::{BigDecimal, FromPrimitive};
    use crate::parser::select_field::select_field;
    use crate::parser::select_index::select_index;
    use crate::parser::sequence::sequence;
    use combine::parser::char::{char as char_, digit, string};
    use poem_openapi::__private::poem::EndpointExt;
    use crate::{InferredType, Range};
    use super::*;

    enum RightSide {
        RightInclusiveExpr { expr: Expr },
        RightExpr { expr: Expr },
    }

    pub(crate) fn range<Input>() -> impl Parser<Input, Output = Range>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        (
            // Following the range syntax with semantics of rust
            // Allows space on either side of the dots, but not in between dots (or . and .=)
            attempt(select_field()).or(attempt(select_index())).or(identifier()).or(pos_num()).skip(spaces()),
            char_('.'),
            char_('.'),
            optional((string("=").skip(spaces()),  identifier().or(pos_num()).skip(spaces())).map(|(_, expr)| RightSide::RightInclusiveExpr { expr }).or(
                spaces().with(identifier().or(pos_num()).skip(spaces())).map(|expr| RightSide::RightExpr { expr }),
            )),
        )
            .map(|(a, b, c, d): (Expr, _, _, Option<RightSide>)| {
                match d {
                    Some(RightSide::RightInclusiveExpr { expr }) => {
                        Range::RangeInclusive { from: a, to: expr }
                    }
                    Some(RightSide::RightExpr { expr }) => {
                        Range::Range { from: a, to: expr }
                    }
                    None => {
                        Range::RangeFrom { from: a }
                    }
                }
            })
    }

    pub(crate) fn pos_num<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        many1(digit()).and_then(|s: String| match s {
            s if s.len() > 0 => s
                .parse::<u64>()
                .map(|num: u64| Expr::number(BigDecimal::from_u64(num).unwrap(), None, InferredType::U64))
                .map_err(|_| RibParseError::Message("Unable to parse number".to_string())),
            _ => Err(RibParseError::Message("Unable to parse number".to_string())),
        })
    }

    pub(crate) fn base_expr<Input>() -> impl Parser<Input, Output = Expr>
    where
        Input: combine::Stream<Token = char>,
        RibParseError: Into<
            <Input::Error as ParseError<Input::Token, Input::Range, Input::Position>>::StreamError,
        >,
        Input::Position: GetSourcePosition,
    {
        choice((
            attempt(sequence()),
            attempt(select_field()),
            attempt(select_index()),
            attempt(identifier()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use bigdecimal::FromPrimitive;
    use combine::stream::position;
    use combine::EasyParser;
    use test_r::test;
    use crate::parser::range::range;
    use crate::Range;

    #[test]
    fn test_range_inclusive() {

        // All kind of ranges that `rust` supports
        let range1 = "1..=2"; // no spaces on both ends
        let range2 = "1 ..= 2"; // space on both end
        let range3      = "1 ..=2"; // space on left
        let range4 = "1..=   2"; // space on right

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;
        let result3 = range().easy_parse(position::Stream::new(range3)).unwrap().0;
        let result4 = range().easy_parse(position::Stream::new(range4)).unwrap().0;

        assert_eq!(result1, Range::RangeInclusive {
            from: crate::Expr::number(
                bigdecimal::BigDecimal::from_u64(1).unwrap(),
                None,
                crate::InferredType::U64
            ),
            to: crate::Expr::number(
                bigdecimal::BigDecimal::from_u64(2).unwrap(),
                None,
                crate::InferredType::U64
            )
        });
    }

    #[test]
    fn test_range() {

        // All kind of ranges that `rust` supports
        let range1 = "1..2"; // no spaces on both ends
        let range2 = "1 .. 2"; // space on both end
        let range3      = "1 ..2"; // space on left
        let range4 = "1.. 2"; // space on right

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;
        let result3 = range().easy_parse(position::Stream::new(range3)).unwrap().0;
        let result4 = range().easy_parse(position::Stream::new(range4)).unwrap().0;

        assert_eq!(result1, Range::Range {
            from: crate::Expr::number(
                bigdecimal::BigDecimal::from_u64(1).unwrap(),
                None,
                crate::InferredType::U64
            ),
            to: crate::Expr::number(
                bigdecimal::BigDecimal::from_u64(2).unwrap(),
                None,
                crate::InferredType::U64
            )
        });
    }
}
