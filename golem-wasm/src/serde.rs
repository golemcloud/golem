use crate::WitValue;
use serde::{Deserialize, Deserializer, Serialize};

impl<'de> Deserialize<'de> for WitValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let binary = Vec::<u8>::deserialize(deserializer)?;
        desert_rust::deserialize(&binary).map_err(serde::de::Error::custom)
    }
}

impl Serialize for WitValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let binary = desert_rust::serialize_to_byte_vec(self).map_err(serde::ser::Error::custom)?;
        binary.serialize(serializer)
    }
}
