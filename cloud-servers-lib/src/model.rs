use golem_common::model::{ShardId, TemplateId, WorkerStatus};
use golem_common::proto::golem::{
    Pod as GrpcPod, RoutingTable as GrpcRoutingTable, RoutingTableEntry as GrpcRoutingTableEntry,
};
use http::Uri;
use poem_openapi::{NewType, Object, Union};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap; // TODO?? Is this here
                               // TODO; Should ww split errors - that golem error has InvalidAccountId

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct WorkerCreationRequest {
    pub name: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    NewType,
)]
pub struct TemplateName(pub String);

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct VersionedTemplateId {
    pub template_id: TemplateId,
    pub version: i32,
}

impl VersionedTemplateId {
    pub fn slug(&self) -> String {
        format!("{}#{}", self.template_id.0, self.version)
    }
}

impl TryFrom<golem_common::proto::golem::VersionedTemplateId> for VersionedTemplateId {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::VersionedTemplateId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            template_id: value.template_id.ok_or("Missing template_id")?.try_into()?,
            version: value.version,
        })
    }
}

impl From<VersionedTemplateId> for golem_common::proto::golem::VersionedTemplateId {
    fn from(value: VersionedTemplateId) -> Self {
        Self {
            template_id: Some(value.template_id.into()),
            version: value.version,
        }
    }
}

impl std::fmt::Display for VersionedTemplateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.template_id, self.version)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct UserTemplateId {
    pub versioned_template_id: VersionedTemplateId,
}

impl TryFrom<golem_common::proto::golem::UserTemplateId> for UserTemplateId {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::UserTemplateId) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_template_id: value
                .versioned_template_id
                .ok_or("Missing versioned_template_id")?
                .try_into()?,
        })
    }
}

impl From<UserTemplateId> for golem_common::proto::golem::UserTemplateId {
    fn from(value: UserTemplateId) -> Self {
        Self {
            versioned_template_id: Some(value.versioned_template_id.into()),
        }
    }
}

impl UserTemplateId {
    pub fn slug(&self) -> String {
        format!("{}:user", self.versioned_template_id.slug())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ProtectedTemplateId {
    pub versioned_template_id: VersionedTemplateId,
}

impl ProtectedTemplateId {
    pub fn slug(&self) -> String {
        format!("{}:protected", self.versioned_template_id.slug())
    }
}

impl TryFrom<golem_common::proto::golem::ProtectedTemplateId> for ProtectedTemplateId {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::ProtectedTemplateId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_template_id: value
                .versioned_template_id
                .ok_or("Missing versioned_template_id")?
                .try_into()?,
        })
    }
}

impl From<ProtectedTemplateId> for golem_common::proto::golem::ProtectedTemplateId {
    fn from(value: ProtectedTemplateId) -> Self {
        Self {
            versioned_template_id: Some(value.versioned_template_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct Empty {}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeResult {
    pub ok: Option<Box<Type>>,
    pub err: Option<Box<Type>>,
}

impl<'de> Deserialize<'de> for TypeResult {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (ok, err) = <(Option<Type>, Option<Type>)>::deserialize(deserializer)?;

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
        let ok: Option<Type> = self.ok.clone().map(|t| *t);
        let err: Option<Type> = self.err.clone().map(|t| *t);
        let pair: (Option<Type>, Option<Type>) = (ok, err);
        <(Option<Type>, Option<Type>)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct NameTypePair {
    pub name: String,
    pub typ: Box<Type>,
}

impl<'de> Deserialize<'de> for NameTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, Type)>::deserialize(deserializer)?;

        Ok(Self {
            name,
            typ: Box::new(typ),
        })
    }
}

impl Serialize for NameTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let pair: (String, Type) = (self.name.clone(), *self.typ.clone());
        <(String, Type)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct NameOptionTypePair {
    pub name: String,
    pub typ: Option<Box<Type>>,
}

impl<'de> Deserialize<'de> for NameOptionTypePair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let (name, typ) = <(String, Option<Type>)>::deserialize(deserializer)?;

        Ok(Self {
            name,
            typ: typ.map(Box::new),
        })
    }
}

impl Serialize for NameOptionTypePair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let typ: Option<Type> = self.typ.clone().map(|t| *t);
        let pair: (String, Option<Type>) = (self.name.clone(), typ);
        <(String, Option<Type>)>::serialize(&pair, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeVariant {
    pub cases: Vec<NameOptionTypePair>,
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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeOption {
    pub inner: Box<Type>,
}

impl<'de> Deserialize<'de> for TypeOption {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = Type::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeOption {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Type::serialize(&self.inner, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeEnum {
    pub cases: Vec<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeFlags {
    pub cases: Vec<String>,
}

impl<'de> Deserialize<'de> for TypeFlags {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<String>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeFlags {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<String>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeRecord {
    pub cases: Vec<NameTypePair>,
}

impl<'de> Deserialize<'de> for TypeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let cases = <Vec<NameTypePair>>::deserialize(deserializer)?;
        Ok(Self { cases })
    }
}

impl Serialize for TypeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<NameTypePair>>::serialize(&self.cases, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeTuple {
    pub items: Vec<Type>,
}

impl<'de> Deserialize<'de> for TypeTuple {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let items = <Vec<Type>>::deserialize(deserializer)?;
        Ok(Self { items })
    }
}

impl Serialize for TypeTuple {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        <Vec<Type>>::serialize(&self.items, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeList {
    pub inner: Box<Type>,
}

impl<'de> Deserialize<'de> for TypeList {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let t = Type::deserialize(deserializer)?;
        Ok(Self { inner: Box::new(t) })
    }
}

impl Serialize for TypeList {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Type::serialize(&self.inner, serializer)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeStr;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeChr;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeF64;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeF32;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU64;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS64;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU32;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS32;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU16;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS16;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeU8;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeS8;

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

#[derive(Debug, Clone, PartialEq, Eq, Object)]
pub struct TypeBool;

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum Type {
    Variant(TypeVariant),
    Result(TypeResult),
    Option(TypeOption),
    Enum(TypeEnum),
    Flags(TypeFlags),
    Record(TypeRecord),
    Tuple(TypeTuple),
    List(TypeList),
    Str(TypeStr),
    Chr(TypeChr),
    F64(TypeF64),
    F32(TypeF32),
    U64(TypeU64),
    S64(TypeS64),
    U32(TypeU32),
    S32(TypeS32),
    U16(TypeU16),
    S16(TypeS16),
    U8(TypeU8),
    S8(TypeS8),
    Bool(TypeBool),
}

impl TryFrom<golem_common::proto::golem::Type> for Type {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::Type) -> Result<Self, Self::Error> {
        match value.r#type {
            None => Err("Missing type".to_string()),
            Some(golem_common::proto::golem::r#type::Type::Variant(variant)) => {
                Ok(Self::Variant(TypeVariant {
                    cases: variant
                        .cases
                        .into_iter()
                        .map(|case| match case.typ {
                            None => Ok(NameOptionTypePair {
                                name: case.name,
                                typ: None,
                            }),
                            Some(typ) => typ.try_into().map(|t| NameOptionTypePair {
                                name: case.name,
                                typ: Some(Box::new(t)),
                            }),
                        })
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::Result(result)) => {
                let ok = match result.ok {
                    None => None,
                    Some(ok) => Some(Box::new((*ok).try_into()?)),
                };
                let err = match result.err {
                    None => None,
                    Some(err) => Some(Box::new((*err).try_into()?)),
                };

                Ok(Self::Result(TypeResult { ok, err }))
            }
            Some(golem_common::proto::golem::r#type::Type::Option(option)) => {
                Ok(Self::Option(TypeOption {
                    inner: Box::new((*option.elem.ok_or("Missing elem")?).try_into()?),
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::Enum(r#enum)) => {
                Ok(Self::Enum(TypeEnum {
                    cases: r#enum.names,
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::Flags(flags)) => {
                Ok(Self::Flags(TypeFlags { cases: flags.names }))
            }
            Some(golem_common::proto::golem::r#type::Type::Record(record)) => {
                Ok(Self::Record(TypeRecord {
                    cases: record
                        .fields
                        .into_iter()
                        .map(|field| {
                            Ok::<NameTypePair, String>(NameTypePair {
                                name: field.name,
                                typ: Box::new(field.typ.ok_or("Missing typ")?.try_into()?),
                            })
                        })
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::Tuple(tuple)) => {
                Ok(Self::Tuple(TypeTuple {
                    items: tuple
                        .elems
                        .into_iter()
                        .map(|item| item.try_into())
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::List(list)) => {
                Ok(Self::List(TypeList {
                    inner: Box::new((*list.elem.ok_or("Missing elem")?).try_into()?),
                }))
            }
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 12 },
            )) => Ok(Self::Str(TypeStr)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 11 },
            )) => Ok(Self::Chr(TypeChr)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 10 },
            )) => Ok(Self::F64(TypeF64)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 9 },
            )) => Ok(Self::F32(TypeF32)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 8 },
            )) => Ok(Self::U64(TypeU64)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 7 },
            )) => Ok(Self::S64(TypeS64)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 6 },
            )) => Ok(Self::U32(TypeU32)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 5 },
            )) => Ok(Self::S32(TypeS32)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 4 },
            )) => Ok(Self::U16(TypeU16)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 3 },
            )) => Ok(Self::S16(TypeS16)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 2 },
            )) => Ok(Self::U8(TypeU8)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 1 },
            )) => Ok(Self::S8(TypeS8)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive: 0 },
            )) => Ok(Self::Bool(TypeBool)),
            Some(golem_common::proto::golem::r#type::Type::Primitive(
                golem_common::proto::golem::TypePrimitive { primitive },
            )) => Err(format!("Invalid primitive: {}", primitive)),
        }
    }
}

impl From<Type> for golem_common::proto::golem::Type {
    fn from(value: Type) -> Self {
        match value {
            Type::Variant(variant) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Variant(
                    golem_common::proto::golem::TypeVariant {
                        cases: variant
                            .cases
                            .into_iter()
                            .map(|case| golem_common::proto::golem::NameOptionTypePair {
                                name: case.name,
                                typ: case.typ.map(|typ| (*typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Result(result) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Result(Box::new(
                    golem_common::proto::golem::TypeResult {
                        ok: result.ok.map(|ok| Box::new((*ok).into())),
                        err: result.err.map(|err| Box::new((*err).into())),
                    },
                ))),
            },
            Type::Option(option) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Option(Box::new(
                    golem_common::proto::golem::TypeOption {
                        elem: Some(Box::new((*option.inner).into())),
                    },
                ))),
            },
            Type::Enum(r#enum) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Enum(
                    golem_common::proto::golem::TypeEnum {
                        names: r#enum.cases,
                    },
                )),
            },
            Type::Flags(flags) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Flags(
                    golem_common::proto::golem::TypeFlags { names: flags.cases },
                )),
            },
            Type::Record(record) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Record(
                    golem_common::proto::golem::TypeRecord {
                        fields: record
                            .cases
                            .into_iter()
                            .map(|case| golem_common::proto::golem::NameTypePair {
                                name: case.name,
                                typ: Some((*case.typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Tuple(tuple) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Tuple(
                    golem_common::proto::golem::TypeTuple {
                        elems: tuple.items.into_iter().map(|item| item.into()).collect(),
                    },
                )),
            },
            Type::List(list) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::List(Box::new(
                    golem_common::proto::golem::TypeList {
                        elem: Some(Box::new((*list.inner).into())),
                    },
                ))),
            },
            Type::Str(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 12 },
                )),
            },
            Type::Chr(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 11 },
                )),
            },
            Type::F64(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 10 },
                )),
            },
            Type::F32(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 9 },
                )),
            },
            Type::U64(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 8 },
                )),
            },
            Type::S64(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 7 },
                )),
            },
            Type::U32(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 6 },
                )),
            },
            Type::S32(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 5 },
                )),
            },
            Type::U16(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 4 },
                )),
            },
            Type::S16(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 3 },
                )),
            },
            Type::U8(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 2 },
                )),
            },
            Type::S8(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 1 },
                )),
            },
            Type::Bool(_) => Self {
                r#type: Some(golem_common::proto::golem::r#type::Type::Primitive(
                    golem_common::proto::golem::TypePrimitive { primitive: 0 },
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct FunctionParameter {
    pub name: String,
    pub tpe: Type,
}

impl TryFrom<golem_common::proto::golem::FunctionParameter> for FunctionParameter {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::FunctionParameter) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            tpe: value.tpe.ok_or("Missing tpe")?.try_into()?,
        })
    }
}

impl From<FunctionParameter> for golem_common::proto::golem::FunctionParameter {
    fn from(value: FunctionParameter) -> Self {
        Self {
            name: value.name,
            tpe: Some(value.tpe.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct FunctionResult {
    pub name: Option<String>,
    pub tpe: Type,
}

impl TryFrom<golem_common::proto::golem::FunctionResult> for FunctionResult {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::FunctionResult) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            tpe: value.tpe.ok_or("Missing tpe")?.try_into()?,
        })
    }
}

impl From<FunctionResult> for golem_common::proto::golem::FunctionResult {
    fn from(value: FunctionResult) -> Self {
        Self {
            name: value.name,
            tpe: Some(value.tpe.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct ExportInstance {
    pub name: String,
    pub functions: Vec<ExportFunction>,
}

impl TryFrom<golem_common::proto::golem::ExportInstance> for ExportInstance {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::ExportInstance) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            functions: value
                .functions
                .into_iter()
                .map(|function| function.try_into())
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ExportInstance> for golem_common::proto::golem::ExportInstance {
    fn from(value: ExportInstance) -> Self {
        Self {
            name: value.name,
            functions: value
                .functions
                .into_iter()
                .map(|function| function.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct ExportFunction {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub results: Vec<FunctionResult>,
}

impl TryFrom<golem_common::proto::golem::ExportFunction> for ExportFunction {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::ExportFunction) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            parameters: value
                .parameters
                .into_iter()
                .map(|parameter| parameter.try_into())
                .collect::<Result<_, _>>()?,
            results: value
                .results
                .into_iter()
                .map(|result| result.try_into())
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<ExportFunction> for golem_common::proto::golem::ExportFunction {
    fn from(value: ExportFunction) -> Self {
        Self {
            name: value.name,
            parameters: value
                .parameters
                .into_iter()
                .map(|parameter| parameter.into())
                .collect(),
            results: value
                .results
                .into_iter()
                .map(|result| result.into())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum Export {
    Instance(ExportInstance),
    Function(ExportFunction),
}

impl TryFrom<golem_common::proto::golem::Export> for Export {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::Export) -> Result<Self, Self::Error> {
        match value.export {
            None => Err("Missing export".to_string()),
            Some(golem_common::proto::golem::export::Export::Instance(instance)) => {
                Ok(Self::Instance(instance.try_into()?))
            }
            Some(golem_common::proto::golem::export::Export::Function(function)) => {
                Ok(Self::Function(function.try_into()?))
            }
        }
    }
}

impl From<Export> for golem_common::proto::golem::Export {
    fn from(value: Export) -> Self {
        match value {
            Export::Instance(instance) => Self {
                export: Some(golem_common::proto::golem::export::Export::Instance(
                    instance.into(),
                )),
            },
            Export::Function(function) => Self {
                export: Some(golem_common::proto::golem::export::Export::Function(
                    function.into(),
                )),
            },
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct VersionedName {
    pub name: String,
    pub version: String,
}

impl From<golem_common::proto::golem::VersionedName> for VersionedName {
    fn from(value: golem_common::proto::golem::VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

impl From<VersionedName> for golem_common::proto::golem::VersionedName {
    fn from(value: VersionedName) -> Self {
        Self {
            name: value.name,
            version: value.version,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct ProducerField {
    pub name: String,
    pub values: Vec<VersionedName>,
}

impl From<golem_common::proto::golem::ProducerField> for ProducerField {
    fn from(value: golem_common::proto::golem::ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

impl From<ProducerField> for golem_common::proto::golem::ProducerField {
    fn from(value: ProducerField) -> Self {
        Self {
            name: value.name,
            values: value.values.into_iter().map(|value| value.into()).collect(),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct Producers {
    pub fields: Vec<ProducerField>,
}

impl From<golem_common::proto::golem::Producers> for Producers {
    fn from(value: golem_common::proto::golem::Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

impl From<Producers> for golem_common::proto::golem::Producers {
    fn from(value: Producers) -> Self {
        Self {
            fields: value.fields.into_iter().map(|field| field.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct TemplateMetadata {
    pub exports: Vec<Export>,
    pub producers: Vec<Producers>,
}

impl TemplateMetadata {
    pub fn instances(&self) -> Vec<ExportInstance> {
        let mut instances = vec![];
        for export in self.exports.clone() {
            if let Export::Instance(instance) = export {
                instances.push(instance.clone())
            }
        }
        instances
    }

    pub fn functions(&self) -> Vec<ExportFunction> {
        let mut functions = vec![];
        for export in self.exports.clone() {
            if let Export::Function(function) = export {
                functions.push(function.clone())
            }
        }
        functions
    }

    pub fn function_by_name(&self, name: &String) -> Option<ExportFunction> {
        let last_slash = name.rfind('/');

        match last_slash {
            None => self.functions().iter().find(|f| f.name == *name).cloned(),
            Some(last_slash_index) => {
                let (instance_name, function_name) = name.split_at(last_slash_index);
                let function_name = &function_name[1..];

                self.instances()
                    .iter()
                    .find(|instance| instance.name == instance_name)
                    .and_then(|instance| {
                        instance
                            .functions
                            .iter()
                            .find(|f| f.name == function_name)
                            .cloned()
                    })
            }
        }
    }
}

impl TryFrom<golem_common::proto::golem::TemplateMetadata> for TemplateMetadata {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::TemplateMetadata) -> Result<Self, Self::Error> {
        Ok(Self {
            exports: value
                .exports
                .into_iter()
                .map(|export| export.try_into())
                .collect::<Result<_, _>>()?,
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
        })
    }
}

impl From<TemplateMetadata> for golem_common::proto::golem::TemplateMetadata {
    fn from(value: TemplateMetadata) -> Self {
        Self {
            exports: value
                .exports
                .into_iter()
                .map(|export| export.into())
                .collect(),
            producers: value
                .producers
                .into_iter()
                .map(|producer| producer.into())
                .collect(),
        }
    }
}

// NOTE: different from golem_common::model::WorkerId because of field name annotations
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerId {
    pub template_id: TemplateId,
    pub worker_name: Id,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
    NewType,
)]
pub struct Id(String);

impl TryFrom<String> for Id {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let _ = valid_id(value.as_str())?;
        Ok(Self(value))
    }
}

impl WorkerId {
    pub fn new(template_id: TemplateId, worker_name: String) -> Result<Self, &'static str> {
        Ok(Self {
            template_id,
            worker_name: worker_name.try_into()?,
        })
    }
}

fn valid_id(identifier: &str) -> Result<&str, &'static str> {
    let length = identifier.len();
    if !(1..=100).contains(&length) {
        Err("Identifier must be between 1 and 100 characters")
    } else if identifier.contains(' ') {
        Err("Identifier must not contain spaces")
    } else if !identifier
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        Err("Identifier must contain only alphanumeric characters, underscores, and dashes")
    } else if identifier.starts_with('-') {
        Err("Identifier must not start with a dash")
    } else {
        Ok(identifier)
    }
}

impl From<golem_common::model::WorkerId> for WorkerId {
    fn from(value: golem_common::model::WorkerId) -> Self {
        Self {
            template_id: value.template_id,
            worker_name: Id(value.worker_name),
        }
    }
}

impl From<WorkerId> for golem_common::model::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            template_id: value.template_id,
            worker_name: value.worker_name.0,
        }
    }
}

impl TryFrom<golem_common::proto::golem::WorkerId> for WorkerId {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::WorkerId) -> Result<Self, Self::Error> {
        let worker_name: Id = value.name.try_into().map_err(String::from)?;

        Ok(Self {
            template_id: value.template_id.ok_or("Missing template_id")?.try_into()?,
            worker_name,
        })
    }
}

impl From<WorkerId> for golem_common::proto::golem::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            template_id: Some(value.template_id.into()),
            name: value.worker_name.0,
        }
    }
}

impl std::fmt::Display for WorkerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.template_id, self.worker_name.0)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct VersionedWorkerId {
    pub worker_id: WorkerId,
    pub template_version_used: i32,
}

impl TryFrom<golem_common::proto::golem::VersionedWorkerId> for VersionedWorkerId {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::VersionedWorkerId) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            template_version_used: value.template_version,
        })
    }
}

impl From<VersionedWorkerId> for golem_common::proto::golem::VersionedWorkerId {
    fn from(value: VersionedWorkerId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            template_version: value.template_version_used,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CompleteParameters {
    pub oplog_idx: i32,
    pub data: Vec<u8>,
}

impl From<CompleteParameters> for golem_common::proto::golem::CompleteParameters {
    fn from(value: CompleteParameters) -> Self {
        Self {
            oplog_idx: value.oplog_idx,
            data: value.data,
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct PromiseId {
    pub worker_id: WorkerId,
    pub oplog_idx: i32,
}

impl TryFrom<golem_common::proto::golem::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::PromiseId) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            oplog_idx: value.oplog_idx,
        })
    }
}

impl From<PromiseId> for golem_common::proto::golem::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            oplog_idx: value.oplog_idx,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorInvalidRequest {
    pub details: String,
}

impl From<golem_common::proto::golem::InvalidRequest> for GolemErrorInvalidRequest {
    fn from(value: golem_common::proto::golem::InvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}
impl From<GolemErrorInvalidRequest> for golem_common::proto::golem::InvalidRequest {
    fn from(value: GolemErrorInvalidRequest) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorWorkerAlreadyExists {
    pub worker_id: WorkerId,
}

impl TryFrom<golem_common::proto::golem::WorkerAlreadyExists> for GolemErrorWorkerAlreadyExists {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::WorkerAlreadyExists,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerAlreadyExists> for golem_common::proto::golem::WorkerAlreadyExists {
    fn from(value: GolemErrorWorkerAlreadyExists) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorWorkerNotFound {
    pub worker_id: WorkerId,
}

impl TryFrom<golem_common::proto::golem::WorkerNotFound> for GolemErrorWorkerNotFound {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::WorkerNotFound) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorWorkerNotFound> for golem_common::proto::golem::WorkerNotFound {
    fn from(value: GolemErrorWorkerNotFound) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorWorkerCreationFailed {
    pub worker_id: WorkerId,
    pub details: String,
}

impl TryFrom<golem_common::proto::golem::WorkerCreationFailed> for GolemErrorWorkerCreationFailed {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::WorkerCreationFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
            details: value.details,
        })
    }
}

impl From<GolemErrorWorkerCreationFailed> for golem_common::proto::golem::WorkerCreationFailed {
    fn from(value: GolemErrorWorkerCreationFailed) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorFailedToResumeWorker {
    pub worker_id: WorkerId,
}

impl TryFrom<golem_common::proto::golem::FailedToResumeWorker> for GolemErrorFailedToResumeWorker {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::FailedToResumeWorker,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value
                .worker_id
                .ok_or("Missing field: worker_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorFailedToResumeWorker> for golem_common::proto::golem::FailedToResumeWorker {
    fn from(value: GolemErrorFailedToResumeWorker) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorTemplateDownloadFailed {
    pub template_id: VersionedTemplateId,
    pub reason: String,
}

impl TryFrom<golem_common::proto::golem::TemplateDownloadFailed>
    for GolemErrorTemplateDownloadFailed
{
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::TemplateDownloadFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            template_id: VersionedTemplateId {
                template_id: value
                    .template_id
                    .ok_or("Missing field: template_id")?
                    .try_into()?,
                version: value.template_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorTemplateDownloadFailed> for golem_common::proto::golem::TemplateDownloadFailed {
    fn from(value: GolemErrorTemplateDownloadFailed) -> Self {
        let template_version = value.template_id.version;
        let template_id = golem_common::proto::golem::TemplateId {
            value: Some(value.template_id.template_id.0.into()),
        };
        Self {
            template_id: Some(template_id),
            template_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorTemplateParseFailed {
    pub template_id: VersionedTemplateId,
    pub reason: String,
}

impl TryFrom<golem_common::proto::golem::TemplateParseFailed> for GolemErrorTemplateParseFailed {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::TemplateParseFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            template_id: VersionedTemplateId {
                template_id: value
                    .template_id
                    .ok_or("Missing field: template_id")?
                    .try_into()?,
                version: value.template_version,
            },
            reason: value.reason,
        })
    }
}

impl From<GolemErrorTemplateParseFailed> for golem_common::proto::golem::TemplateParseFailed {
    fn from(value: GolemErrorTemplateParseFailed) -> Self {
        let template_version = value.template_id.version;
        let template_id = golem_common::proto::golem::TemplateId {
            value: Some(value.template_id.template_id.0.into()),
        };
        Self {
            template_id: Some(template_id),
            template_version,
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorGetLatestVersionOfTemplateFailed {
    pub template_id: TemplateId,
    pub reason: String,
}

impl TryFrom<golem_common::proto::golem::GetLatestVersionOfTemplateFailed>
    for GolemErrorGetLatestVersionOfTemplateFailed
{
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::GetLatestVersionOfTemplateFailed,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            template_id: value
                .template_id
                .ok_or("Missing field: template_id")?
                .try_into()?,
            reason: value.reason,
        })
    }
}

impl From<GolemErrorGetLatestVersionOfTemplateFailed>
    for golem_common::proto::golem::GetLatestVersionOfTemplateFailed
{
    fn from(value: GolemErrorGetLatestVersionOfTemplateFailed) -> Self {
        Self {
            template_id: Some(value.template_id.into()),
            reason: value.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorPromiseNotFound {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_common::proto::golem::PromiseNotFound> for GolemErrorPromiseNotFound {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::PromiseNotFound) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseNotFound> for golem_common::proto::golem::PromiseNotFound {
    fn from(value: GolemErrorPromiseNotFound) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorPromiseDropped {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_common::proto::golem::PromiseDropped> for GolemErrorPromiseDropped {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::PromiseDropped) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseDropped> for golem_common::proto::golem::PromiseDropped {
    fn from(value: GolemErrorPromiseDropped) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorPromiseAlreadyCompleted {
    pub promise_id: PromiseId,
}

impl TryFrom<golem_common::proto::golem::PromiseAlreadyCompleted>
    for GolemErrorPromiseAlreadyCompleted
{
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::PromiseAlreadyCompleted,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            promise_id: value
                .promise_id
                .ok_or("Missing field: promise_id")?
                .try_into()?,
        })
    }
}

impl From<GolemErrorPromiseAlreadyCompleted>
    for golem_common::proto::golem::PromiseAlreadyCompleted
{
    fn from(value: GolemErrorPromiseAlreadyCompleted) -> Self {
        Self {
            promise_id: Some(value.promise_id.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorInterrupted {
    pub recover_immediately: bool,
}

impl From<golem_common::proto::golem::Interrupted> for GolemErrorInterrupted {
    fn from(value: golem_common::proto::golem::Interrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

impl From<GolemErrorInterrupted> for golem_common::proto::golem::Interrupted {
    fn from(value: GolemErrorInterrupted) -> Self {
        Self {
            recover_immediately: value.recover_immediately,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorParamTypeMismatch {}

impl From<golem_common::proto::golem::ParamTypeMismatch> for GolemErrorParamTypeMismatch {
    fn from(_value: golem_common::proto::golem::ParamTypeMismatch) -> Self {
        Self {}
    }
}

impl From<GolemErrorParamTypeMismatch> for golem_common::proto::golem::ParamTypeMismatch {
    fn from(_value: GolemErrorParamTypeMismatch) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorNoValueInMessage {}

impl From<golem_common::proto::golem::NoValueInMessage> for GolemErrorNoValueInMessage {
    fn from(_value: golem_common::proto::golem::NoValueInMessage) -> Self {
        Self {}
    }
}

impl From<GolemErrorNoValueInMessage> for golem_common::proto::golem::NoValueInMessage {
    fn from(_value: GolemErrorNoValueInMessage) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorValueMismatch {
    pub details: String,
}

impl From<golem_common::proto::golem::ValueMismatch> for GolemErrorValueMismatch {
    fn from(value: golem_common::proto::golem::ValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorValueMismatch> for golem_common::proto::golem::ValueMismatch {
    fn from(value: GolemErrorValueMismatch) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorUnexpectedOplogEntry {
    pub expected: String,
    pub got: String,
}

impl From<golem_common::proto::golem::UnexpectedOplogEntry> for GolemErrorUnexpectedOplogEntry {
    fn from(value: golem_common::proto::golem::UnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

impl From<GolemErrorUnexpectedOplogEntry> for golem_common::proto::golem::UnexpectedOplogEntry {
    fn from(value: GolemErrorUnexpectedOplogEntry) -> Self {
        Self {
            expected: value.expected,
            got: value.got,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorRuntimeError {
    pub details: String,
}

impl From<golem_common::proto::golem::RuntimeError> for GolemErrorRuntimeError {
    fn from(value: golem_common::proto::golem::RuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorRuntimeError> for golem_common::proto::golem::RuntimeError {
    fn from(value: GolemErrorRuntimeError) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GolemErrorInvalidShardId {
    pub shard_id: ShardId,
    pub shard_ids: std::collections::HashSet<ShardId>,
}

impl TryFrom<golem_common::proto::golem::InvalidShardId> for GolemErrorInvalidShardId {
    type Error = String;
    fn try_from(value: golem_common::proto::golem::InvalidShardId) -> Result<Self, Self::Error> {
        Ok(Self {
            shard_id: value.shard_id.ok_or("Missing field: shard_id")?.into(),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        })
    }
}

impl From<GolemErrorInvalidShardId> for golem_common::proto::golem::InvalidShardId {
    fn from(value: GolemErrorInvalidShardId) -> Self {
        Self {
            shard_id: Some(value.shard_id.into()),
            shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorPreviousInvocationFailed {}

impl From<golem_common::proto::golem::PreviousInvocationFailed>
    for GolemErrorPreviousInvocationFailed
{
    fn from(_value: golem_common::proto::golem::PreviousInvocationFailed) -> Self {
        Self {}
    }
}
impl From<GolemErrorPreviousInvocationFailed>
    for golem_common::proto::golem::PreviousInvocationFailed
{
    fn from(_value: GolemErrorPreviousInvocationFailed) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorPreviousInvocationExited {}

impl From<golem_common::proto::golem::PreviousInvocationExited>
    for GolemErrorPreviousInvocationExited
{
    fn from(_value: golem_common::proto::golem::PreviousInvocationExited) -> Self {
        Self {}
    }
}

impl From<GolemErrorPreviousInvocationExited>
    for golem_common::proto::golem::PreviousInvocationExited
{
    fn from(_value: GolemErrorPreviousInvocationExited) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorUnknown {
    pub details: String,
}

impl From<golem_common::proto::golem::UnknownError> for GolemErrorUnknown {
    fn from(value: golem_common::proto::golem::UnknownError) -> Self {
        Self {
            details: value.details,
        }
    }
}

impl From<GolemErrorUnknown> for golem_common::proto::golem::UnknownError {
    fn from(value: GolemErrorUnknown) -> Self {
        Self {
            details: value.details,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct GolemErrorInvalidAccount {}

impl From<golem_common::proto::golem::InvalidAccount> for GolemErrorInvalidAccount {
    fn from(_value: golem_common::proto::golem::InvalidAccount) -> Self {
        Self {}
    }
}

impl From<GolemErrorInvalidAccount> for golem_common::proto::golem::InvalidAccount {
    fn from(_value: GolemErrorInvalidAccount) -> Self {
        Self {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct InvokeParameters {
    pub params: serde_json::value::Value,
}

impl InvokeParameters {
    pub fn as_json_string(&self) -> String {
        serde_json::to_string(&self.params).unwrap()
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct DeleteWorkerResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct InvokeResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct InterruptResponse {}

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize, Object,
)]
pub struct ResumeResponse {}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct WorkerMetadata {
    pub worker_id: WorkerId,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub status: WorkerStatus,
    pub template_version: i32,
    pub retry_count: i32,
}

impl TryFrom<golem_common::proto::golem::WorkerMetadata> for WorkerMetadata {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::WorkerMetadata) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            args: value.args,
            env: value.env,
            status: value.status.try_into()?,
            template_version: value.template_version,
            retry_count: value.retry_count,
        })
    }
}

impl From<WorkerMetadata> for golem_common::proto::golem::WorkerMetadata {
    fn from(value: WorkerMetadata) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            account_id: None, //FIXME: This should be removed
            args: value.args,
            env: value.env,
            status: value.status.into(),
            template_version: value.template_version,
            retry_count: value.retry_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
pub struct InvokeResult {
    pub result: serde_json::value::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum GolemError {
    InvalidRequest(GolemErrorInvalidRequest),
    WorkerAlreadyExists(GolemErrorWorkerAlreadyExists),
    WorkerNotFound(GolemErrorWorkerNotFound),
    WorkerCreationFailed(GolemErrorWorkerCreationFailed),
    FailedToResumeWorker(GolemErrorFailedToResumeWorker),
    TemplateDownloadFailed(GolemErrorTemplateDownloadFailed),
    TemplateParseFailed(GolemErrorTemplateParseFailed),
    GetLatestVersionOfTemplateFailed(GolemErrorGetLatestVersionOfTemplateFailed),
    PromiseNotFound(GolemErrorPromiseNotFound),
    PromiseDropped(GolemErrorPromiseDropped),
    PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted),
    Interrupted(GolemErrorInterrupted),
    ParamTypeMismatch(GolemErrorParamTypeMismatch),
    NoValueInMessage(GolemErrorNoValueInMessage),
    ValueMismatch(GolemErrorValueMismatch),
    UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry),
    RuntimeError(GolemErrorRuntimeError),
    InvalidShardId(GolemErrorInvalidShardId),
    PreviousInvocationFailed(GolemErrorPreviousInvocationFailed),
    PreviousInvocationExited(GolemErrorPreviousInvocationExited),
    Unknown(GolemErrorUnknown),
    InvalidAccount(GolemErrorInvalidAccount),
}

impl TryFrom<golem_common::proto::golem::GolemError> for GolemError {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::GolemError) -> Result<Self, Self::Error> {
        match value.error {
            Some(golem_common::proto::golem::golem_error::Error::InvalidRequest(err)) => {
                Ok(GolemError::InvalidRequest(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::WorkerAlreadyExists(err)) => {
                Ok(GolemError::WorkerAlreadyExists(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::WorkerNotFound(err)) => {
                Ok(GolemError::WorkerNotFound(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::WorkerCreationFailed(err)) => {
                Ok(GolemError::WorkerCreationFailed(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::FailedToResumeWorker(err)) => {
                Ok(GolemError::FailedToResumeWorker(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::TemplateDownloadFailed(err)) => {
                Ok(GolemError::TemplateDownloadFailed(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::TemplateParseFailed(err)) => {
                Ok(GolemError::TemplateParseFailed(err.try_into()?))
            }
            Some(
                golem_common::proto::golem::golem_error::Error::GetLatestVersionOfTemplateFailed(
                    err,
                ),
            ) => Ok(GolemError::GetLatestVersionOfTemplateFailed(
                err.try_into()?,
            )),
            Some(golem_common::proto::golem::golem_error::Error::PromiseNotFound(err)) => {
                Ok(GolemError::PromiseNotFound(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::PromiseDropped(err)) => {
                Ok(GolemError::PromiseDropped(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::PromiseAlreadyCompleted(err)) => {
                Ok(GolemError::PromiseAlreadyCompleted(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::Interrupted(err)) => {
                Ok(GolemError::Interrupted(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::ParamTypeMismatch(err)) => {
                Ok(GolemError::ParamTypeMismatch(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::NoValueInMessage(err)) => {
                Ok(GolemError::NoValueInMessage(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::ValueMismatch(err)) => {
                Ok(GolemError::ValueMismatch(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::UnexpectedOplogEntry(err)) => {
                Ok(GolemError::UnexpectedOplogEntry(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::RuntimeError(err)) => {
                Ok(GolemError::RuntimeError(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::InvalidShardId(err)) => {
                Ok(GolemError::InvalidShardId(err.try_into()?))
            }
            Some(golem_common::proto::golem::golem_error::Error::PreviousInvocationFailed(err)) => {
                Ok(GolemError::PreviousInvocationFailed(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::PreviousInvocationExited(err)) => {
                Ok(GolemError::PreviousInvocationExited(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::Unknown(err)) => {
                Ok(GolemError::Unknown(err.into()))
            }
            Some(golem_common::proto::golem::golem_error::Error::InvalidAccount(err)) => {
                Ok(GolemError::InvalidAccount(err.into()))
            }
            None => Err("Missing field: error".to_string()),
        }
    }
}

impl From<GolemError> for golem_common::proto::golem::GolemError {
    fn from(error: GolemError) -> Self {
        match error {
            GolemError::InvalidRequest(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::InvalidRequest(err.into())),
                }
            },
            GolemError::WorkerAlreadyExists(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::WorkerAlreadyExists(err.into())),
                }
            },
            GolemError::WorkerNotFound(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::WorkerNotFound(err.into())),
                }
            },
            GolemError::WorkerCreationFailed(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::WorkerCreationFailed(err.into())),
                }
            },
            GolemError::FailedToResumeWorker(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::FailedToResumeWorker(err.into())),
                }
            },
            GolemError::TemplateDownloadFailed(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::TemplateDownloadFailed(err.into())),
                }
            },
            GolemError::TemplateParseFailed(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::TemplateParseFailed(err.into())),
                }
            },
            GolemError::GetLatestVersionOfTemplateFailed(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::GetLatestVersionOfTemplateFailed(err.into())),
                }
            },
            GolemError::PromiseNotFound(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::PromiseNotFound(err.into())),
                }
            },
            GolemError::PromiseDropped(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::PromiseDropped(err.into())),
                }
            },
            GolemError::PromiseAlreadyCompleted(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::PromiseAlreadyCompleted(err.into())),
                }
            },
            GolemError::Interrupted(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::Interrupted(err.into())),
                }
            },
            GolemError::ParamTypeMismatch(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::ParamTypeMismatch(err.into())),
                }
            },
            GolemError::NoValueInMessage(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::NoValueInMessage(err.into())),
                }
            },
            GolemError::ValueMismatch(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::ValueMismatch(err.into())),
                }
            },
            GolemError::UnexpectedOplogEntry(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::UnexpectedOplogEntry(err.into())),
                }
            },
            GolemError::RuntimeError(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::RuntimeError(err.into())),
                }
            },
            GolemError::InvalidShardId(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::InvalidShardId(err.into())),
                }
            },
            GolemError::PreviousInvocationFailed(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::PreviousInvocationFailed(err.into())),
                }
            },
            GolemError::PreviousInvocationExited(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::PreviousInvocationExited(err.into())),
                }
            },
            GolemError::Unknown(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::Unknown(err.into())),
                }
            },
            GolemError::InvalidAccount(err) => {
                golem_common::proto::golem::GolemError {
                    error: Some(golem_common::proto::golem::golem_error::Error::InvalidAccount(err.into())),
                }
            }
        }
    }
}

#[derive(Object)]
pub struct GolemErrorBody {
    pub golem_error: GolemError,
}

impl TryFrom<golem_common::proto::golem::GolemError> for GolemErrorBody {
    type Error = String;

    fn try_from(
        value: golem_common::proto::golem::GolemError,
    ) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            golem_error: value.try_into()?,
        })
    }
}

#[derive(Object)]
pub struct ErrorsBody {
    pub errors: Vec<String>,
}

#[derive(Object)]
pub struct ErrorBody {
    pub error: String,
}

impl From<golem_common::proto::golem::ErrorBody> for ErrorBody {
    fn from(value: golem_common::proto::golem::ErrorBody) -> Self {
        Self { error: value.error }
    }
}

impl From<golem_common::proto::golem::ErrorsBody> for ErrorsBody {
    fn from(value: golem_common::proto::golem::ErrorsBody) -> Self {
        Self {
            errors: value.errors,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct Template {
    pub versioned_template_id: VersionedTemplateId,
    pub user_template_id: UserTemplateId,
    pub protected_template_id: ProtectedTemplateId,
    pub template_name: TemplateName,
    pub template_size: i32,
    pub metadata: TemplateMetadata,
}

impl TryFrom<golem_common::proto::golem::Template> for Template {
    type Error = String;

    fn try_from(value: golem_common::proto::golem::Template) -> Result<Self, Self::Error> {
        Ok(Self {
            versioned_template_id: value
                .versioned_template_id
                .ok_or("Missing versioned_template_id")?
                .try_into()?,
            user_template_id: value
                .user_template_id
                .ok_or("Missing user_template_id")?
                .try_into()?,
            protected_template_id: value
                .protected_template_id
                .ok_or("Missing protected_template_id")?
                .try_into()?,
            template_name: TemplateName(value.template_name),
            template_size: value.template_size,
            metadata: value.metadata.ok_or("Missing metadata")?.try_into()?,
        })
    }
}

impl From<Template> for golem_common::proto::golem::Template {
    fn from(value: Template) -> Self {
        Self {
            versioned_template_id: Some(value.versioned_template_id.into()),
            user_template_id: Some(value.user_template_id.into()),
            protected_template_id: Some(value.protected_template_id.into()),
            template_name: value.template_name.0,
            template_size: value.template_size,
            metadata: Some(value.metadata.into()),
            project_id: None, // FIXME: Probably we need OSS grpc type
        }
    }
}

impl Template {
    pub fn next_version(self) -> Self {
        let new_version = VersionedTemplateId {
            template_id: self.versioned_template_id.template_id,
            version: self.versioned_template_id.version + 1,
        };
        Self {
            versioned_template_id: new_version.clone(),
            user_template_id: UserTemplateId {
                versioned_template_id: new_version.clone(),
            },
            protected_template_id: ProtectedTemplateId {
                versioned_template_id: new_version,
            },
            ..self
        }
    }
}

#[derive(Clone)]
pub struct NumberOfShards {
    pub value: usize,
}

#[derive(Clone, Debug)]
pub struct Pod {
    host: String,
    port: u16,
}

impl Pod {
    pub fn uri(&self) -> Uri {
        Uri::builder()
            .scheme("http")
            .authority(format!("{}:{}", self.host, self.port).as_str())
            .path_and_query("/")
            .build()
            .expect("Failed to build URI")
    }
}

impl From<GrpcPod> for Pod {
    fn from(value: GrpcPod) -> Self {
        Self {
            host: value.host,
            port: value.port as u16,
        }
    }
}

#[derive(Clone)]
pub struct RoutingTable {
    pub number_of_shards: NumberOfShards,
    shard_assignments: HashMap<ShardId, Pod>,
}

impl RoutingTable {
    pub fn lookup(&self, worker_id: &WorkerId) -> Option<&Pod> {
        self.shard_assignments.get(&ShardId::from_worker_id(
            &worker_id.clone().into(),
            self.number_of_shards.value,
        ))
    }
}

impl From<GrpcRoutingTable> for RoutingTable {
    fn from(value: GrpcRoutingTable) -> Self {
        Self {
            number_of_shards: NumberOfShards {
                value: value.number_of_shards as usize,
            },
            shard_assignments: value
                .shard_assignments
                .into_iter()
                .map(RoutingTableEntry::from)
                .map(|routing_table_entry| (routing_table_entry.shard_id, routing_table_entry.pod))
                .collect(),
        }
    }
}

pub struct RoutingTableEntry {
    shard_id: ShardId,
    pod: Pod,
}

impl From<GrpcRoutingTableEntry> for RoutingTableEntry {
    fn from(value: GrpcRoutingTableEntry) -> Self {
        Self {
            shard_id: value.shard_id.unwrap().into(),
            pod: value.pod.unwrap().into(),
        }
    }
}
