use std::cmp::Ordering;
use std::fmt;
use std::fmt::{Debug, Display, Formatter};
use std::hash::{Hash, Hasher};

#[derive(Clone, Default)]
pub struct SourceSpan {
    start: SourcePosition,
    end: SourcePosition,
}

impl SourceSpan {
    pub fn new(start: SourcePosition, end: SourcePosition) -> SourceSpan {
        SourceSpan { start, end }
    }

    pub fn merge(&self, right: SourceSpan) -> SourceSpan {
        SourceSpan {
            start: self.start.clone(),
            end: right.end,
        }
    }
}

/// These instances are important as source span shouldn't take part in any comparison
/// or hashing or order of `Expr`.
impl PartialEq for SourceSpan {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for SourceSpan {}

impl Hash for SourceSpan {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

#[allow(clippy::non_canonical_partial_ord_impl)]
impl PartialOrd for SourceSpan {
    fn partial_cmp(&self, _: &Self) -> Option<Ordering> {
        Some(Ordering::Equal)
    }
}

impl Ord for SourceSpan {
    fn cmp(&self, _: &Self) -> Ordering {
        Ordering::Equal
    }
}

impl Debug for SourceSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "<SourceSpan>")
    }
}

impl Display for SourceSpan {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Start: [{}], End: [{}]", self.start, self.end)
    }
}

#[derive(Clone, Default)]
pub struct SourcePosition {
    pub line: i32,
    pub column: i32,
}

impl Display for SourcePosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Line: {}, Column: {}", self.line, self.column)
    }
}

pub trait GetSourcePosition {
    fn get_source_position(&self) -> SourcePosition;
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
impl GetSourcePosition for combine::stream::position::SourcePosition {
    fn get_source_position(&self) -> SourcePosition {
        SourcePosition {
            line: self.line,
            column: self.column,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{rib_source_span::SourceSpan, Expr};
    use test_r::test;

    #[derive(Debug, PartialEq)]
    struct RibSourceSpanDebug {
        start_line: i32,
        start_column: i32,
        end_line: i32,
        end_column: i32,
    }

    impl From<SourceSpan> for RibSourceSpanDebug {
        fn from(span: SourceSpan) -> Self {
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
          "${x} ${y}""#,
        )
        .unwrap();

        let mut parent_span = None;

        let spans: Vec<RibSourceSpanDebug> = if let Expr::ExprBlock {
            exprs, source_span, ..
        } = rib_expr
        {
            parent_span = Some(source_span);
            exprs.iter().map(|expr| expr.source_span().into()).collect()
        } else {
            vec![]
        };

        let expected_spans = vec![
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
                start_line: 6,
                start_column: 11,
                end_line: 6,
                end_column: 22,
            },
        ];

        let expected_parent_span = RibSourceSpanDebug {
            start_line: 2,
            start_column: 11,
            end_line: 6,
            end_column: 22,
        };

        assert_eq!(spans, expected_spans);

        assert_eq!(parent_span.map(|x| x.into()), Some(expected_parent_span));
    }
}
