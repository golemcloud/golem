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
    pub fn start_line(&self) -> i32 {
        self.start.line
    }

    pub fn end_line(&self) -> i32 {
        self.end.line
    }

    pub fn eq(&self, other: &SourceSpan) -> bool {
        self.start_line() == other.start_line()
            && self.start_column() == other.start_column()
            && self.end_line() == other.end_line()
            && self.end_column() == other.end_column()
    }

    pub fn start_column(&self) -> i32 {
        self.start.column
    }

    pub fn end_column(&self) -> i32 {
        self.end.column
    }

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
        write!(
            f,
            "at line {}, column {}",
            self.start_line(),
            self.start_column()
        )
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

impl SourcePosition {
    pub fn new(line: i32, column: i32) -> SourcePosition {
        SourcePosition { line, column }
    }
}

impl Display for SourcePosition {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Line: {}, Column: {}", self.line, self.column)
    }
}

pub trait GetSourcePosition {
    fn get_source_position(&self) -> SourcePosition;
}

// With the below instance, we are able to access line number and column number
// within each parser. Also, this implies the input to Rib parser has to be (always)
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
    struct SourceSpanDebug {
        start_line: i32,
        start_column: i32,
        end_line: i32,
        end_column: i32,
    }

    impl From<SourceSpan> for SourceSpanDebug {
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

        let mut block_span: Option<SourceSpanDebug> = None;

        let line_spans: Vec<SourceSpanDebug> = if let Expr::ExprBlock {
            exprs, source_span, ..
        } = rib_expr
        {
            block_span = Some(source_span.into());

            exprs.iter().map(|expr| expr.source_span().into()).collect()
        } else {
            vec![]
        };

        let expected_line_spans = vec![
            SourceSpanDebug {
                start_line: 2,
                start_column: 11,
                end_line: 3,
                end_column: 18,
            },
            SourceSpanDebug {
                start_line: 4,
                start_column: 11,
                end_line: 5,
                end_column: 18,
            },
            SourceSpanDebug {
                start_line: 6,
                start_column: 11,
                end_line: 6,
                end_column: 22,
            },
        ];

        let expected_parent_span = SourceSpanDebug {
            start_line: 2,
            start_column: 11,
            end_line: 6,
            end_column: 22,
        };

        assert_eq!(line_spans, expected_line_spans);
        assert_eq!(block_span, Some(expected_parent_span));
    }
}
