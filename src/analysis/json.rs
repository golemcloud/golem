// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

impl<'de> Deserialize<'de> for TypeResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (ok, err) = <(Option<AnalysedType>, Option<AnalysedType>)>::deserialize(deserializer)?;

        Ok(Self {
            ok: ok.map(Box::new),
            err: err.map(Box::new),
        })
    }
}

impl Serialize for TypeResult {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ok: Option<AnalysedType> = self.ok.clone().map(|t| *t);
        let err: Option<AnalysedType> = self.err.clone().map(|t| *t);
        let pair: (Option<AnalysedType>, Option<AnalysedType>) = (ok, err);
        <(Option<AnalysedType>, Option<AnalysedType>)>::serialize(&pair, serializer)
    }
}

impl<'de> Deserialize<'de> for NameTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, AnalysedType)>::deserialize(deserializer)?;

        Ok(Self { name, typ })
    }
}

impl Serialize for NameTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let pair: (String, AnalysedType) = (self.name.clone(), self.typ.clone());
        <(String, AnalysedType)>::serialize(&pair, serializer)
    }
}

impl<'de> Deserialize<'de> for NameOptionTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, Option<AnalysedType>)>::deserialize(deserializer)?;

        Ok(Self { name, typ })
    }
}

impl Serialize for NameOptionTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let typ: Option<AnalysedType> = self.typ.clone();
        let pair: (String, Option<AnalysedType>) = (self.name.clone(), typ);
        <(String, Option<AnalysedType>)>::serialize(&pair, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeVariant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<NameOptionTypePair>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeVariant {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<NameOptionTypePair>>::serialize(&self.cases, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = AnalysedType::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeOption {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        AnalysedType::serialize(&self.inner, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeEnum {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeEnum {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<String>>::serialize(&self.cases, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self { names: cases })
    }
}

impl Serialize for TypeFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<String>>::serialize(&self.names, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<NameTypePair>>::deserialize(deserializer)?;
        Ok(Self { fields: cases })
    }
}

impl Serialize for TypeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<NameTypePair>>::serialize(&self.fields, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeTuple {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let items = <Vec<AnalysedType>>::deserialize(deserializer)?;
        Ok(Self { items })
    }
}

impl Serialize for TypeTuple {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<AnalysedType>>::serialize(&self.items, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = AnalysedType::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        AnalysedType::serialize(&self.inner, serializer)
    }
}

impl<'de> Deserialize<'de> for TypeStr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeStr),
            serde_json::Value::Null => Ok(TypeStr),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeStr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeChr {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeChr),
            serde_json::Value::Null => Ok(TypeChr),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeChr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeF64),
            serde_json::Value::Null => Ok(TypeF64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeF64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeF32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeF32),
            serde_json::Value::Null => Ok(TypeF32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeF32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU64),
            serde_json::Value::Null => Ok(TypeU64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeS64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS64),
            serde_json::Value::Null => Ok(TypeS64),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeU32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU32),
            serde_json::Value::Null => Ok(TypeU32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeS32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS32),
            serde_json::Value::Null => Ok(TypeS32),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeU16 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU16),
            serde_json::Value::Null => Ok(TypeU16),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU16 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeS16 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS16),
            serde_json::Value::Null => Ok(TypeS16),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS16 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeU8 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeU8),
            serde_json::Value::Null => Ok(TypeU8),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeU8 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeS8 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeS8),
            serde_json::Value::Null => Ok(TypeS8),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeS8 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}

impl<'de> Deserialize<'de> for TypeBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match value {
            serde_json::Value::Object(map) if map.is_empty() => Ok(TypeBool),
            serde_json::Value::Null => Ok(TypeBool),
            _ => Err(serde::de::Error::custom("Expected empty object")),
        }
    }
}

impl Serialize for TypeBool {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serde_json::Value::serialize(
            &serde_json::Value::Object(serde_json::Map::new()),
            serializer,
        )
    }
}
