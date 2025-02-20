use combine::stream::position::SourcePosition;

#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct RibSourceSpan {
    start: RibSourcePosition,
    end: RibSourcePosition,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct RibSourcePosition {
    pub line: i32,
    pub column: i32,
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