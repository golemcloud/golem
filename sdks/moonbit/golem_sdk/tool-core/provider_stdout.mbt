///|
pub(all) suberror ToolStreamError {
  Cancelled
  Abandoned
  ResourceExhausted
  Failed(String)
} derive(Debug, Eq)
