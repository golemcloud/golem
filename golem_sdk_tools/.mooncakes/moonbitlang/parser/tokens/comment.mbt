///|
pub(all) struct Comment {
  content : String
  kind : CommentKind
  mut consumed_by_docstring : Bool
} derive(Show, Eq)

///|
pub(all) enum CommentKind {
  InlineTrailing
  Ownline(leading_blank_line~ : Bool, trailing_blank_line~ : Bool)
} derive(Show, Eq)
