use std::fmt::{Display, Formatter};

use serde::Deserialize;

#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmptyAuthCtx();

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

#[derive(
    Default, Debug, Clone, PartialEq, Eq, Hash, bincode::Encode, bincode::Decode, Deserialize,
)]
pub struct EmptyNamespace();

impl Display for EmptyNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "")
    }
}

impl TryFrom<String> for EmptyNamespace {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            Ok(EmptyNamespace::default())
        } else {
            Err("Failed to parse empty namespace".to_string())
        }
    }
}
