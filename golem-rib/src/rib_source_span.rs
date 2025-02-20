use combine::stream::position::SourcePosition;
use std::cmp::Ordering;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};

#[derive(Clone, Default)]
pub struct RibSourceSpan {
    start: RibSourcePosition,
    end: RibSourcePosition,
}

impl RibSourceSpan {
    pub fn new(start: RibSourcePosition, end: RibSourcePosition) -> RibSourceSpan {
        RibSourceSpan { start, end }
    }
}

/// These instances are important as source span shouldn't take part in any comparison
/// or hashing or order of `Expr`.
impl PartialEq for RibSourceSpan {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for RibSourceSpan {}

impl Hash for RibSourceSpan {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl PartialOrd for RibSourceSpan {
    fn partial_cmp(&self, _: &Self) -> Option<Ordering> {
        Some(Ordering::Equal)
    }
}

impl Ord for RibSourceSpan {
    fn cmp(&self, _: &Self) -> Ordering {
        Ordering::Equal
    }
}

impl Debug for RibSourceSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", "<SourceSpan>")
    }
}

impl Display for RibSourceSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Start: [{}], End: [{}]", self.start, self.end)
    }
}

#[derive(Clone, Default)]
pub struct RibSourcePosition {
    pub line: i32,
    pub column: i32,
}

impl Display for RibSourcePosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Line: {}, Column: {}", self.line, self.column)
    }
}

pub trait GetSourcePosition {
    fn get_source_position(&self) -> RibSourcePosition;
}

// Rib parsers are polymorphic with `Input` and this was done to allow
// certain parsers such as `function_name` to compile/work properly. This may change in future `combine` versions
// making things simple.
// This implies associative `Input::Position`  should have line number and column number
// (unlike `PointerOffset` accessible through translate position in a simple input `&str`)
// to be converted to `RibSourcePosition` and with the below instance, we are able to access line number and column number
// within each parser.
// Also, this implies the input to Rib parser has to be (always)
// `combine::stream::position::Stream` where `Input::Position` is `stream::position::SourcePosition`.
// See `Expr::from_text` functionality where we form this input.
impl GetSourcePosition for SourcePosition {
    fn get_source_position(&self) -> RibSourcePosition {
        RibSourcePosition {
            line: self.line,
            column: self.column,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{rib_source_span::RibSourceSpan, Expr};
    use test_r::test;

    #[derive(Debug, PartialEq)]
    struct RibSourceSpanDebug {
        start_line: i32,
        start_column: i32,
        end_line: i32,
        end_column: i32,
    }

    impl From<RibSourceSpan> for RibSourceSpanDebug {
        fn from(span: RibSourceSpan) -> Self {
            Self {
                start_line: span.start.line,
                start_column: span.start.column,
                end_line: span.end.line,
                end_column: span.end.column,
            }
        }
    }

    #[test]
    fn test_rib_source_span() {
        let rib_expr = Expr::from_text(
            r#"
          let x =
            "foo";
          let y =
            "bar";
          "${x} ${y}"
        "#,
        )
        .unwrap();

        let spans: Vec<RibSourceSpanDebug> = if let Expr::ExprBlock { exprs, .. } = rib_expr {
            exprs.iter().map(|expr| expr.source_span().into()).collect()
        } else {
            vec![]
        };

        let expected = vec![
            RibSourceSpanDebug {
                start_line: 2,
                start_column: 11,
                end_line: 3,
                end_column: 18,
            },
            RibSourceSpanDebug {
                start_line: 4,
                start_column: 11,
                end_line: 5,
                end_column: 18,
            },
            RibSourceSpanDebug {
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
            },
        ];

        assert_eq!(spans, expected);
    }
}
