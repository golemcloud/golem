use crate::WitValue;
use serde::{Deserialize, Deserializer, Serialize};

impl<'de> Deserialize<'de> for WitValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let binary = Vec::<u8>::deserialize(deserializer)?;
        bincode::decode_from_slice(&binary, bincode::config::standard())
            .map_err(serde::de::Error::custom)
            .map(|(value, _)| value)
    }
}

impl Serialize for WitValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let binary = bincode::encode_to_vec(self, bincode::config::standard())
            .map_err(serde::ser::Error::custom)?;
        binary.serialize(serializer)
    }
}
