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

use combine::parser::char::{char as char_, spaces, string};
use combine::{attempt, many1, optional, ParseError, Parser};

use crate::expr::Expr;
use crate::parser::errors::RibParseError;
use crate::parser::identifier::identifier;
use crate::parser::select_field::select_field;
use crate::parser::select_index::select_index;
use crate::rib_source_span::GetSourcePosition;
use crate::Range;

pub fn range<Input>() -> impl Parser<Input, Output = Range>
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
        optional(
            attempt(select_field())
                .or(attempt(select_index()))
                .or(identifier())
                .or(internal::pos_num())
                .skip(spaces()),
        ),
        char_('.'),
        char_('.'),
        optional(
            (
                string("=").skip(spaces()),
                identifier().or(internal::pos_num()).skip(spaces()),
            )
                .map(|(_, expr)| internal::RightSide::RightInclusiveExpr { expr })
                .or(spaces()
                    .with(identifier().or(internal::pos_num()).skip(spaces()))
                    .map(|expr| internal::RightSide::RightExpr { expr })),
        ),
    )
        .map(
            |(a, _, _, d): (Option<Expr>, _, _, Option<internal::RightSide>)| match (a, d) {
                (Some(left_side), Some(right_side)) => match right_side {
                    internal::RightSide::RightInclusiveExpr { expr: right_side } => {
                        Range::RangeInclusive {
                            from: left_side,
                            to: right_side,
                        }
                    }
                    internal::RightSide::RightExpr { expr: right_side } => Range::Range {
                        from: left_side,
                        to: right_side,
                    },
                },

                (Some(left_side), None) => Range::RangeFrom { from: left_side },

                (None, Some(right_side)) => match right_side {
                    internal::RightSide::RightInclusiveExpr { expr: right_side } => {
                        Range::RangeToInclusive { to: right_side }
                    }
                    internal::RightSide::RightExpr { expr: right_side } => {
                        Range::RangeTo { to: right_side }
                    }
                },

                (None, None) => Range::RangeFull,
            },
        )
}

mod internal {
    use crate::parser::RibParseError;
    use crate::rib_source_span::GetSourcePosition;
    use crate::{Expr, InferredType};
    use bigdecimal::{BigDecimal, FromPrimitive};
    use combine::parser::char::digit;
    use combine::{many1, ParseError, Parser};

    pub(crate) enum RightSide {
        RightInclusiveExpr { expr: Expr },
        RightExpr { expr: Expr },
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
                .map(|num: u64| {
                    Expr::number(BigDecimal::from_u64(num).unwrap(), None, InferredType::U64)
                })
                .map_err(|_| RibParseError::Message("Unable to parse number".to_string())),
            _ => Err(RibParseError::Message("Unable to parse number".to_string())),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::range::range;
    use crate::Range;
    use bigdecimal::FromPrimitive;
    use combine::stream::position;
    use combine::EasyParser;
    use test_r::test;

    #[test]
    fn test_range() {
        // All kind of ranges that `rust` supports
        let range1 = "1..2"; // no spaces on both ends
        let range2 = "1 .. 2"; // space on both end
        let range3 = "1 ..2"; // space on left
        let range4 = "1.. 2"; // space on right
        let invalid_range = "1. .2";

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;
        let result3 = range().easy_parse(position::Stream::new(range3)).unwrap().0;
        let result4 = range().easy_parse(position::Stream::new(range4)).unwrap().0;
        let result5 = range().easy_parse(position::Stream::new(invalid_range));

        assert!(result1 == result2 && result2 == result3 && result3 == result4);
        assert!(result5.is_err());

        assert_eq!(
            result1,
            Range::Range {
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
            }
        );
    }

    #[test]
    fn test_range_inclusive() {
        // All kind of ranges that `rust` supports
        let range1 = "1..=2"; // no spaces on both ends
        let range2 = "1 ..= 2"; // space on both end
        let range3 = "1 ..=2"; // space on left
        let range4 = "1..=   2"; // space on right
        let invalid_range = "1.. =2";

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;
        let result3 = range().easy_parse(position::Stream::new(range3)).unwrap().0;
        let result4 = range().easy_parse(position::Stream::new(range4)).unwrap().0;
        let result5 = range().easy_parse(position::Stream::new(invalid_range));

        assert!(result1 == result2 && result2 == result3 && result3 == result4);
        assert!(result5.is_err());
        assert_eq!(
            result1,
            Range::RangeInclusive {
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
            }
        );
    }

    #[test]
    fn test_range_from() {
        // All kind of ranges that `rust` supports
        let range1 = "1.."; // no spaces on both ends
        let range2 = "1 .."; // space on both end

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;

        assert_eq!(result1, result2);

        assert_eq!(
            result1,
            Range::RangeFrom {
                from: crate::Expr::number(
                    bigdecimal::BigDecimal::from_u64(1).unwrap(),
                    None,
                    crate::InferredType::U64
                )
            }
        );
    }

    #[test]
    fn test_range_to() {
        // All kind of ranges that `rust` supports
        let range1 = "..2"; // no spaces on both ends
        let range2 = ".. 2"; // space on both end

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;

        assert_eq!(
            result1,
            Range::RangeTo {
                to: crate::Expr::number(
                    bigdecimal::BigDecimal::from_u64(2).unwrap(),
                    None,
                    crate::InferredType::U64
                )
            }
        );
    }

    #[test]
    fn test_range_to_inclusive() {
        // All kind of ranges that `rust` supports
        let range1 = "..=2"; // no spaces on both ends
        let range2 = "..= 2";
        let invalid = ".. =2";

        let result1 = range().easy_parse(position::Stream::new(range1)).unwrap().0;
        let result2 = range().easy_parse(position::Stream::new(range2)).unwrap().0;
        let result3 = range().easy_parse(position::Stream::new(invalid));

        assert_eq!(result1, result2);
        assert!(result3.is_err());

        assert_eq!(
            result1,
            Range::RangeToInclusive {
                to: crate::Expr::number(
                    bigdecimal::BigDecimal::from_u64(2).unwrap(),
                    None,
                    crate::InferredType::U64
                )
            }
        );
    }
}
