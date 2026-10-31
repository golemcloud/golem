///|
pub(all) suberror ToolStreamError {
  Cancelled
  Abandoned
  ResourceExhausted
  Failed(String)
} derive(Debug, Eq)

///|
pub extend ToolStreamError with Debug::{to_repr}

///|
pub extend ToolStreamError with Eq::{equal, not_equal}
