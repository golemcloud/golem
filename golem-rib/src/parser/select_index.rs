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

use crate::rib_source_span::GetSourcePosition;
use bigdecimal::ToPrimitive;
use combine::{ParseError, Parser};

#[cfg(test)]
mod tests {
    use bigdecimal::BigDecimal;
    use test_r::test;

    use crate::expr::*;

    #[test]
    fn test_select_index() {
        let input = "foo[0]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::untyped_number(BigDecimal::from(0)),
                None
            ))
        );
    }

    #[test]
    fn test_recursive_select_index() {
        let input = "foo[0][1]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::select_dynamic(
                    Expr::identifier_global("foo", None),
                    Expr::untyped_number(BigDecimal::from(0)),
                    None
                ),
                Expr::untyped_number(BigDecimal::from(1)),
                None
            ))
        );
    }

    #[test]
    fn test_select_dynamic_index_1() {
        let input = "foo[bar]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::identifier_global("bar", None),
                None
            ))
        );
    }

    #[test]
    fn test_select_dynamic_index_2() {
        let input = "foo[1..2]";
        let result = Expr::from_text(input);
        assert_eq!(
            result,
            Ok(Expr::select_dynamic(
                Expr::identifier_global("foo", None),
                Expr::range(
                    Expr::untyped_number(BigDecimal::from(1)),
                    Expr::untyped_number(BigDecimal::from(2))
                ),
                None
            ))
        );
    }
}
