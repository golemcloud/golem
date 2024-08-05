use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::{AnalysedResourceId, AnalysedResourceMode};
use poem_openapi::{Enum, Object, Union};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use rib::ParsedFunctionName;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union, Encode, Decode)]
#[oai(discriminator_name = "type", one_of = true)]
pub enum Export {
    Instance(ExportInstance),
    Function(ExportFunction),
}

impl Export {
    pub fn function_names(&self) -> Vec<String> {
        match self {
            Export::Instance(instance) => instance
                .functions
                .iter()
                .map(|function| format!("{}.{{{}}}", instance.name, function.name))
                .collect(),
            Export::Function(function) => vec![function.name.clone()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Object, Encode, Decode)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Enum, Encode, Decode)]
pub enum ResourceMode {
    Borrowed,
    Owned,
}

impl From<AnalysedResourceMode> for ResourceMode {
    fn from(value: AnalysedResourceMode) -> Self {
        match value {
            AnalysedResourceMode::Borrowed => ResourceMode::Borrowed,
            AnalysedResourceMode::Owned => ResourceMode::Owned,
        }
    }
}

impl From<ResourceMode> for AnalysedResourceMode {
    fn from(value: ResourceMode) -> Self {
        match value {
            ResourceMode::Borrowed => AnalysedResourceMode::Borrowed,
            ResourceMode::Owned => AnalysedResourceMode::Owned,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct TypeHandle {
    resource_id: u64,
    mode: ResourceMode,
}

impl TryFrom<golem_wasm_rpc::protobuf::TypeHandle> for TypeHandle {
    type Error = String;

    fn try_from(value: golem_wasm_rpc::protobuf::TypeHandle) -> Result<Self, Self::Error> {
        Ok(Self {
            resource_id: value.resource_id,
            mode: match golem_wasm_rpc::protobuf::ResourceMode::try_from(value.mode) {
                Ok(golem_wasm_rpc::protobuf::ResourceMode::Borrowed) => ResourceMode::Borrowed,
                Ok(golem_wasm_rpc::protobuf::ResourceMode::Owned) => ResourceMode::Owned,
                Err(_) => Err("Invalid mode".to_string())?,
            },
        })
    }
}

impl From<TypeHandle> for golem_wasm_rpc::protobuf::TypeHandle {
    fn from(value: TypeHandle) -> Self {
        Self {
            resource_id: value.resource_id,
            mode: match value.mode {
                ResourceMode::Borrowed => golem_wasm_rpc::protobuf::ResourceMode::Borrowed as i32,
                ResourceMode::Owned => golem_wasm_rpc::protobuf::ResourceMode::Owned as i32,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Union, Encode, Decode)]
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
    Handle(TypeHandle),
}

impl TryFrom<golem_wasm_rpc::protobuf::Type> for Type {
    type Error = String;

    fn try_from(value: golem_wasm_rpc::protobuf::Type) -> Result<Self, Self::Error> {
        match value.r#type {
            None => Err("Missing type".to_string()),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Variant(variant)) => {
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
            Some(golem_wasm_rpc::protobuf::r#type::Type::Result(result)) => {
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
            Some(golem_wasm_rpc::protobuf::r#type::Type::Option(option)) => {
                Ok(Self::Option(TypeOption {
                    inner: Box::new((*option.elem.ok_or("Missing elem")?).try_into()?),
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Enum(r#enum)) => {
                Ok(Self::Enum(TypeEnum {
                    cases: r#enum.names,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Flags(flags)) => {
                Ok(Self::Flags(TypeFlags { cases: flags.names }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::Record(record)) => {
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
            Some(golem_wasm_rpc::protobuf::r#type::Type::Tuple(tuple)) => {
                Ok(Self::Tuple(TypeTuple {
                    items: tuple
                        .elems
                        .into_iter()
                        .map(|item| item.try_into())
                        .collect::<Result<_, _>>()?,
                }))
            }
            Some(golem_wasm_rpc::protobuf::r#type::Type::List(list)) => Ok(Self::List(TypeList {
                inner: Box::new((*list.elem.ok_or("Missing elem")?).try_into()?),
            })),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 12 },
            )) => Ok(Self::Str(TypeStr)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 11 },
            )) => Ok(Self::Chr(TypeChr)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 10 },
            )) => Ok(Self::F64(TypeF64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 9 },
            )) => Ok(Self::F32(TypeF32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 8 },
            )) => Ok(Self::U64(TypeU64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 7 },
            )) => Ok(Self::S64(TypeS64)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 6 },
            )) => Ok(Self::U32(TypeU32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 5 },
            )) => Ok(Self::S32(TypeS32)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 4 },
            )) => Ok(Self::U16(TypeU16)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 3 },
            )) => Ok(Self::S16(TypeS16)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 2 },
            )) => Ok(Self::U8(TypeU8)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 1 },
            )) => Ok(Self::S8(TypeS8)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive: 0 },
            )) => Ok(Self::Bool(TypeBool)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                golem_wasm_rpc::protobuf::TypePrimitive { primitive },
            )) => Err(format!("Invalid primitive: {}", primitive)),
            Some(golem_wasm_rpc::protobuf::r#type::Type::Handle(handle)) => {
                Ok(Self::Handle(handle.try_into()?))
            }
        }
    }
}

impl From<Type> for golem_wasm_rpc::protobuf::Type {
    fn from(value: Type) -> Self {
        match value {
            Type::Variant(variant) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Variant(
                    golem_wasm_rpc::protobuf::TypeVariant {
                        cases: variant
                            .cases
                            .into_iter()
                            .map(|case| golem_wasm_rpc::protobuf::NameOptionTypePair {
                                name: case.name,
                                typ: case.typ.map(|typ| (*typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Result(result) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Result(Box::new(
                    golem_wasm_rpc::protobuf::TypeResult {
                        ok: result.ok.map(|ok| Box::new((*ok).into())),
                        err: result.err.map(|err| Box::new((*err).into())),
                    },
                ))),
            },
            Type::Option(option) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Option(Box::new(
                    golem_wasm_rpc::protobuf::TypeOption {
                        elem: Some(Box::new((*option.inner).into())),
                    },
                ))),
            },
            Type::Enum(r#enum) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Enum(
                    golem_wasm_rpc::protobuf::TypeEnum {
                        names: r#enum.cases,
                    },
                )),
            },
            Type::Flags(flags) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Flags(
                    golem_wasm_rpc::protobuf::TypeFlags { names: flags.cases },
                )),
            },
            Type::Record(record) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Record(
                    golem_wasm_rpc::protobuf::TypeRecord {
                        fields: record
                            .cases
                            .into_iter()
                            .map(|case| golem_wasm_rpc::protobuf::NameTypePair {
                                name: case.name,
                                typ: Some((*case.typ).into()),
                            })
                            .collect(),
                    },
                )),
            },
            Type::Tuple(tuple) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Tuple(
                    golem_wasm_rpc::protobuf::TypeTuple {
                        elems: tuple.items.into_iter().map(|item| item.into()).collect(),
                    },
                )),
            },
            Type::List(list) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::List(Box::new(
                    golem_wasm_rpc::protobuf::TypeList {
                        elem: Some(Box::new((*list.inner).into())),
                    },
                ))),
            },
            Type::Str(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 12 },
                )),
            },
            Type::Chr(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 11 },
                )),
            },
            Type::F64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 10 },
                )),
            },
            Type::F32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 9 },
                )),
            },
            Type::U64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 8 },
                )),
            },
            Type::S64(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 7 },
                )),
            },
            Type::U32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 6 },
                )),
            },
            Type::S32(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 5 },
                )),
            },
            Type::U16(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 4 },
                )),
            },
            Type::S16(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 3 },
                )),
            },
            Type::U8(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 2 },
                )),
            },
            Type::S8(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 1 },
                )),
            },
            Type::Bool(_) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Primitive(
                    golem_wasm_rpc::protobuf::TypePrimitive { primitive: 0 },
                )),
            },
            Type::Handle(handle) => Self {
                r#type: Some(golem_wasm_rpc::protobuf::r#type::Type::Handle(
                    handle.into(),
                )),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct FunctionParameter {
    pub name: String,
    pub typ: Type,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionParameter> for FunctionParameter {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionParameter,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            typ: value.typ.ok_or("Missing typ")?.try_into()?,
        })
    }
}

impl From<FunctionParameter> for golem_api_grpc::proto::golem::component::FunctionParameter {
    fn from(value: FunctionParameter) -> Self {
        Self {
            name: value.name,
            typ: Some(value.typ.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct FunctionResult {
    pub name: Option<String>,
    pub typ: Type,
}

impl TryFrom<golem_api_grpc::proto::golem::component::FunctionResult> for FunctionResult {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::FunctionResult,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            typ: value.typ.ok_or("Missing typ")?.try_into()?,
        })
    }
}

impl From<FunctionResult> for golem_api_grpc::proto::golem::component::FunctionResult {
    fn from(value: FunctionResult) -> Self {
        Self {
            name: value.name,
            typ: Some(value.typ.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct ExportInstance {
    pub name: String,
    pub functions: Vec<ExportFunction>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::ExportInstance> for ExportInstance {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ExportInstance,
    ) -> Result<Self, Self::Error> {
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

impl From<ExportInstance> for golem_api_grpc::proto::golem::component::ExportInstance {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object, Encode, Decode)]
pub struct ExportFunction {
    pub name: String,
    pub parameters: Vec<FunctionParameter>,
    pub results: Vec<FunctionResult>,
}

impl TryFrom<golem_api_grpc::proto::golem::component::ExportFunction> for ExportFunction {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ExportFunction,
    ) -> Result<Self, Self::Error> {
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

impl From<ExportFunction> for golem_api_grpc::proto::golem::component::ExportFunction {
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

impl TryFrom<golem_api_grpc::proto::golem::component::Export> for Export {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Export,
    ) -> Result<Self, Self::Error> {
        match value.export {
            None => Err("Missing export".to_string()),
            Some(golem_api_grpc::proto::golem::component::export::Export::Instance(instance)) => {
                Ok(Self::Instance(instance.try_into()?))
            }
            Some(golem_api_grpc::proto::golem::component::export::Export::Function(function)) => {
                Ok(Self::Function(function.try_into()?))
            }
        }
    }
}

impl From<Export> for golem_api_grpc::proto::golem::component::Export {
    fn from(value: Export) -> Self {
        match value {
            Export::Instance(instance) => Self {
                export: Some(
                    golem_api_grpc::proto::golem::component::export::Export::Instance(
                        instance.into(),
                    ),
                ),
            },
            Export::Function(function) => Self {
                export: Some(
                    golem_api_grpc::proto::golem::component::export::Export::Function(
                        function.into(),
                    ),
                ),
            },
        }
    }
}

pub fn instances(exports: &Vec<Export>) -> Vec<ExportInstance> {
    let mut instances = vec![];
    for export in exports {
        if let Export::Instance(instance) = export {
            instances.push(instance.clone())
        }
    }
    instances
}

pub fn functions(exports: &Vec<Export>) -> Vec<ExportFunction> {
    let mut functions = vec![];
    for export in exports {
        if let Export::Function(function) = export {
            functions.push(function.clone())
        }
    }
    functions
}

pub fn function_by_name(
    exports: &Vec<Export>,
    name: &str,
) -> Result<Option<ExportFunction>, String> {
    let parsed = ParsedFunctionName::parse(name)?;

    match &parsed.site().interface_name() {
        None => Ok(functions(exports).iter().find(|f| f.name == *name).cloned()),
        Some(interface_name) => {
            let exported_function = instances(exports)
                .iter()
                .find(|instance| instance.name == *interface_name)
                .and_then(|instance| {
                    instance
                        .functions
                        .iter()
                        .find(|f| f.name == parsed.function().function_name())
                        .cloned()
                });
            if exported_function.is_none() {
                match parsed.method_as_static() {
                    Some(parsed_static) => Ok(instances(exports)
                        .iter()
                        .find(|instance| instance.name == *interface_name)
                        .and_then(|instance| {
                            instance
                                .functions
                                .iter()
                                .find(|f| f.name == parsed_static.function().function_name())
                                .cloned()
                        })),
                    None => Ok(None),
                }
            } else {
                Ok(exported_function)
            }
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedExport> for Export {
    fn from(value: golem_wasm_ast::analysis::AnalysedExport) -> Self {
        match value {
            golem_wasm_ast::analysis::AnalysedExport::Function(analysed_function) => {
                Export::Function(analysed_function.into())
            }
            golem_wasm_ast::analysis::AnalysedExport::Instance(analysed_instance) => {
                Export::Instance(analysed_instance.into())
            }
        }
    }
}

impl From<Export> for golem_wasm_ast::analysis::AnalysedExport {
    fn from(value: Export) -> Self {
        match value {
            Export::Function(export_function) => {
                golem_wasm_ast::analysis::AnalysedExport::Function(export_function.into())
            }
            Export::Instance(export_instance) => {
                golem_wasm_ast::analysis::AnalysedExport::Instance(export_instance.into())
            }
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunction> for ExportFunction {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunction) -> Self {
        Self {
            name: value.name,
            parameters: value.params.into_iter().map(|p| p.into()).collect(),
            results: value.results.into_iter().map(|r| r.into()).collect(),
        }
    }
}

impl From<ExportFunction> for golem_wasm_ast::analysis::AnalysedFunction {
    fn from(value: ExportFunction) -> Self {
        Self {
            name: value.name,
            params: value.parameters.into_iter().map(|p| p.into()).collect(),
            results: value.results.into_iter().map(|r| r.into()).collect(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedInstance> for ExportInstance {
    fn from(value: golem_wasm_ast::analysis::AnalysedInstance) -> Self {
        Self {
            name: value.name,
            functions: value.funcs.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl From<ExportInstance> for golem_wasm_ast::analysis::AnalysedInstance {
    fn from(value: ExportInstance) -> Self {
        Self {
            name: value.name,
            funcs: value.functions.into_iter().map(|f| f.into()).collect(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunctionParameter> for FunctionParameter {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunctionParameter) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<FunctionParameter> for golem_wasm_ast::analysis::AnalysedFunctionParameter {
    fn from(value: FunctionParameter) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedFunctionResult> for FunctionResult {
    fn from(value: golem_wasm_ast::analysis::AnalysedFunctionResult) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<FunctionResult> for golem_wasm_ast::analysis::AnalysedFunctionResult {
    fn from(value: FunctionResult) -> Self {
        Self {
            name: value.name,
            typ: value.typ.into(),
        }
    }
}

impl From<golem_wasm_ast::analysis::AnalysedType> for Type {
    fn from(value: golem_wasm_ast::analysis::AnalysedType) -> Self {
        match value {
            golem_wasm_ast::analysis::AnalysedType::Bool => Type::Bool(TypeBool),
            golem_wasm_ast::analysis::AnalysedType::S8 => Type::S8(TypeS8),
            golem_wasm_ast::analysis::AnalysedType::U8 => Type::U8(TypeU8),
            golem_wasm_ast::analysis::AnalysedType::S16 => Type::S16(TypeS16),
            golem_wasm_ast::analysis::AnalysedType::U16 => Type::U16(TypeU16),
            golem_wasm_ast::analysis::AnalysedType::S32 => Type::S32(TypeS32),
            golem_wasm_ast::analysis::AnalysedType::U32 => Type::U32(TypeU32),
            golem_wasm_ast::analysis::AnalysedType::S64 => Type::S64(TypeS64),
            golem_wasm_ast::analysis::AnalysedType::U64 => Type::U64(TypeU64),
            golem_wasm_ast::analysis::AnalysedType::F32 => Type::F32(TypeF32),
            golem_wasm_ast::analysis::AnalysedType::F64 => Type::F64(TypeF64),
            golem_wasm_ast::analysis::AnalysedType::Chr => Type::Chr(TypeChr),
            golem_wasm_ast::analysis::AnalysedType::Str => Type::Str(TypeStr),
            golem_wasm_ast::analysis::AnalysedType::List(inner) => Type::List(TypeList {
                inner: Box::new((*inner).into()),
            }),
            golem_wasm_ast::analysis::AnalysedType::Tuple(items) => Type::Tuple(TypeTuple {
                items: items.into_iter().map(|t| t.into()).collect(),
            }),
            golem_wasm_ast::analysis::AnalysedType::Record(cases) => Type::Record(TypeRecord {
                cases: cases
                    .into_iter()
                    .map(|(name, typ)| NameTypePair {
                        name,
                        typ: Box::new(typ.into()),
                    })
                    .collect(),
            }),
            golem_wasm_ast::analysis::AnalysedType::Flags(cases) => {
                Type::Flags(TypeFlags { cases })
            }
            golem_wasm_ast::analysis::AnalysedType::Enum(cases) => Type::Enum(TypeEnum { cases }),
            golem_wasm_ast::analysis::AnalysedType::Option(inner) => Type::Option(TypeOption {
                inner: Box::new((*inner).into()),
            }),
            golem_wasm_ast::analysis::AnalysedType::Result { ok, error } => {
                Type::Result(TypeResult {
                    ok: ok.map(|t| Box::new((*t).into())),
                    err: error.map(|t| Box::new((*t).into())),
                })
            }
            golem_wasm_ast::analysis::AnalysedType::Variant(variants) => {
                Type::Variant(TypeVariant {
                    cases: variants
                        .into_iter()
                        .map(|(name, typ)| NameOptionTypePair {
                            name,
                            typ: typ.map(|t| Box::new(t.into())),
                        })
                        .collect(),
                })
            }
            golem_wasm_ast::analysis::AnalysedType::Resource { id, resource_mode } => {
                Type::Handle(TypeHandle {
                    resource_id: id.value,
                    mode: resource_mode.into(),
                })
            }
        }
    }
}

impl From<Type> for golem_wasm_ast::analysis::AnalysedType {
    fn from(value: Type) -> Self {
        match value {
            Type::Bool(_) => golem_wasm_ast::analysis::AnalysedType::Bool,
            Type::S8(_) => golem_wasm_ast::analysis::AnalysedType::S8,
            Type::U8(_) => golem_wasm_ast::analysis::AnalysedType::U8,
            Type::S16(_) => golem_wasm_ast::analysis::AnalysedType::S16,
            Type::U16(_) => golem_wasm_ast::analysis::AnalysedType::U16,
            Type::S32(_) => golem_wasm_ast::analysis::AnalysedType::S32,
            Type::U32(_) => golem_wasm_ast::analysis::AnalysedType::U32,
            Type::S64(_) => golem_wasm_ast::analysis::AnalysedType::S64,
            Type::U64(_) => golem_wasm_ast::analysis::AnalysedType::U64,
            Type::F32(_) => golem_wasm_ast::analysis::AnalysedType::F32,
            Type::F64(_) => golem_wasm_ast::analysis::AnalysedType::F64,
            Type::Chr(_) => golem_wasm_ast::analysis::AnalysedType::Chr,
            Type::Str(_) => golem_wasm_ast::analysis::AnalysedType::Str,
            Type::List(inner) => {
                let elem_type: golem_wasm_ast::analysis::AnalysedType = (*inner.inner).into();
                golem_wasm_ast::analysis::AnalysedType::List(Box::new(elem_type))
            }
            Type::Tuple(inner) => golem_wasm_ast::analysis::AnalysedType::Tuple(
                inner.items.into_iter().map(|t| t.into()).collect(),
            ),
            Type::Record(inner) => golem_wasm_ast::analysis::AnalysedType::Record(
                inner
                    .cases
                    .into_iter()
                    .map(|case| (case.name, (*case.typ).into()))
                    .collect(),
            ),
            Type::Flags(inner) => golem_wasm_ast::analysis::AnalysedType::Flags(inner.cases),
            Type::Enum(inner) => golem_wasm_ast::analysis::AnalysedType::Enum(inner.cases),
            Type::Option(inner) => {
                golem_wasm_ast::analysis::AnalysedType::Option(Box::new((*inner.inner).into()))
            }
            Type::Result(inner) => golem_wasm_ast::analysis::AnalysedType::Result {
                ok: inner.ok.map(|t| Box::new((*t).into())),
                error: inner.err.map(|t| Box::new((*t).into())),
            },
            Type::Variant(variants) => golem_wasm_ast::analysis::AnalysedType::Variant(
                variants
                    .cases
                    .into_iter()
                    .map(|case| (case.name, case.typ.map(|t| (*t).into())))
                    .collect(),
            ),
            Type::Handle(handle) => golem_wasm_ast::analysis::AnalysedType::Resource {
                id: AnalysedResourceId {
                    value: handle.resource_id,
                },
                resource_mode: handle.mode.into(),
            },
        }
    }
}
