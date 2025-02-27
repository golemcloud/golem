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
use combine::{optional, ParseError, Parser, Stream};

use crate::parser::errors::RibParseError;
use crate::rib_source_span::GetSourcePosition;

// This is range avoiding left recursion
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

#[cfg(test)]
mod tests {
    use crate::{Expr, InferredType};
    use bigdecimal::FromPrimitive;
    use test_r::test;

    #[test]
    fn test_range() {
        // All kind of ranges that `rust` supports
        let range1 = "1..2"; // no spaces on both ends

        let result1 = Expr::from_text(range1).unwrap();

        assert_eq!(
            result1,
            Expr::range(
                Expr::number(
                    bigdecimal::BigDecimal::from_u64(1).unwrap(),
                    None,
                    InferredType::number()
                ),
                Expr::number(
                    bigdecimal::BigDecimal::from_u64(2).unwrap(),
                    None,
                    InferredType::number()
                )
            )
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

        let result1 = Expr::from_text(range1).unwrap();
        let result2 = Expr::from_text(range2).unwrap();
        let result3 = Expr::from_text(range3).unwrap();
        let result4 = Expr::from_text(range4).unwrap();
        let result5 = Expr::from_text(invalid_range);

        assert!(result1 == result2 && result2 == result3 && result3 == result4);
        assert!(result5.is_err());
        assert_eq!(
            result1,
            Expr::range_inclusive(
                Expr::number(
                    bigdecimal::BigDecimal::from_u64(1).unwrap(),
                    None,
                    InferredType::number()
                ),
                Expr::number(
                    bigdecimal::BigDecimal::from_u64(2).unwrap(),
                    None,
                    InferredType::number()
                )
            )
        );
    }

    #[test]
    fn test_range_from() {
        // All kind of ranges that `rust` supports
        let range1 = "1.."; // no spaces on both ends
        let range2 = "1 .."; // space on both end

        let result1 = Expr::from_text(range1).unwrap();
        let result2 = Expr::from_text(range2).unwrap();

        assert_eq!(result1, result2);

        assert_eq!(
            result1,
            Expr::range_from(Expr::number(
                bigdecimal::BigDecimal::from_u64(1).unwrap(),
                None,
                InferredType::number()
            ))
        );
    }
}
