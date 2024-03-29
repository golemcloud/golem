use std::fmt::{Display, Formatter};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx {}

impl Display for EmptyAuthCtx {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EmptyAuthCtx")
    }
}

impl IntoIterator for EmptyAuthCtx {
    type Item = (String, String);
    type IntoIter = std::iter::Empty<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        std::iter::empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, bincode::Encode, bincode::Decode, Deserialize)]
pub struct CommonNamespace(String);

impl Default for CommonNamespace {
    fn default() -> Self {
        CommonNamespace("common".to_string())
    }
}

impl Display for CommonNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
