// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_export]
macro_rules! newtype_uuid {
    ($name:ident, wit_name: $wit_name:literal, wit_owner: $wit_owner:literal $(, $proto_type:path)?) => {
        $crate::newtype_uuid!(@inner $name, $wit_name, $wit_owner $(, $proto_type)?);
    };
    ($name:ident $(, $proto_type:path)?) => {
        $crate::newtype_uuid!(@inner $name, "", "" $(, $proto_type)?);
    };
    (@inner $name:ident, $wit_name:literal, $wit_owner:literal $(, $proto_type:path)?) => {
        #[derive(Copy, Clone, Debug, PartialOrd, Ord, derive_more::FromStr, Eq, Hash, PartialEq)]
        #[derive(serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
        #[serde(transparent)]
        #[cfg_attr(feature = "full", desert(transparent))]
        pub struct $name(pub uuid::Uuid);

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $name {
            pub fn new() -> $name {
                Self(uuid::Uuid::now_v7())
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(value: uuid::Uuid) -> Self {
                Self(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = String;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                let uuid =
                    uuid::Uuid::parse_str(value).map_err(|err| format!("Invalid UUID: {err}"))?;
                Ok(Self(uuid))
            }
        }

        #[cfg(feature = "full")]
        impl poem_openapi::types::Type for $name {
            const IS_REQUIRED: bool = true;
            type RawValueType = Self;
            type RawElementValueType = Self;

            fn name() -> std::borrow::Cow<'static, str> {
                std::borrow::Cow::from(format!("string({})", stringify!($name)))
            }

            fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
                poem_openapi::registry::MetaSchemaRef::Inline(Box::new(
                    poem_openapi::registry::MetaSchema::new_with_format("string", "uuid"),
                ))
            }

            fn as_raw_value(&self) -> Option<&Self::RawValueType> {
                Some(self)
            }

            fn raw_element_iter<'a>(
                &'a self,
            ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
                Box::new(self.as_raw_value().into_iter())
            }
        }

        #[cfg(feature = "full")]
        impl poem_openapi::types::ParseFromParameter for $name {
            fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
                Ok(Self(value.try_into()?))
            }
        }

        #[cfg(feature = "full")]
        impl poem_openapi::types::ParseFromJSON for $name {
            fn parse_from_json(
                value: Option<serde_json::Value>,
            ) -> poem_openapi::types::ParseResult<Self> {
                match value {
                    Some(serde_json::Value::String(s)) => Ok(Self(<uuid::Uuid as std::str::FromStr>::from_str(&s)?)),
                    _ => Err(poem_openapi::types::ParseError::<$name>::custom(format!(
                        "Unexpected representation of {}",
                        stringify!($name)
                    ))),
                }
            }
        }

        #[cfg(feature = "full")]
         impl ::poem_openapi::types::ParseFromMultipartField for $name {
            async fn parse_from_multipart(
                field: Option<::poem::web::Field>,
            ) -> ::poem_openapi::types::ParseResult<Self> {
                <::uuid::Uuid as poem_openapi::types::ParseFromMultipartField>::parse_from_multipart(field).await.map(Self).map_err(|e| e.propagate())
            }
        }

        #[cfg(feature = "full")]
        impl poem_openapi::types::ToJSON for $name {
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::Value::String(self.0.to_string()))
            }
        }

        #[cfg(feature = "full")]
        impl ::poem_openapi::types::ToHeader for $name {
            fn to_header(&self) -> Option<::http::HeaderValue> {
                <::uuid::Uuid as ::poem_openapi::types::ToHeader>::to_header(&self.0)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", &self.0)
            }
        }

        impl golem_wasm::IntoValue for $name {
            fn into_value(self) -> golem_wasm::Value {
                let (hi, lo) = self.0.as_u64_pair();
                golem_wasm::Value::Record(vec![golem_wasm::Value::Record(vec![
                    golem_wasm::Value::U64(hi),
                    golem_wasm::Value::U64(lo),
                ])])
            }

            fn get_type() -> golem_wasm::analysis::AnalysedType {
                let uuid_type = $name::uuid_analysed_type();
                let rec = golem_wasm::analysis::analysed_type::record(vec![
                    golem_wasm::analysis::analysed_type::field("uuid", uuid_type),
                ]);
                let wit_name: &str = $wit_name;
                let wit_owner: &str = $wit_owner;
                let rec = if !wit_name.is_empty() { rec.named(wit_name) } else { rec };
                if !wit_owner.is_empty() { rec.owned(wit_owner) } else { rec }
            }
        }

        impl $name {
            pub fn uuid_analysed_type() -> golem_wasm::analysis::AnalysedType {
                golem_wasm::analysis::analysed_type::record(vec![
                    golem_wasm::analysis::analysed_type::field(
                        "high-bits",
                        golem_wasm::analysis::analysed_type::u64(),
                    ),
                    golem_wasm::analysis::analysed_type::field(
                        "low-bits",
                        golem_wasm::analysis::analysed_type::u64(),
                    ),
                ])
                .named("uuid")
                .owned("golem:core@1.5.0/types")
            }
        }

        impl golem_wasm::FromValue for $name {
            fn from_value(value: golem_wasm::Value) -> Result<$name, String> {
                match value {
                    golem_wasm::Value::Record(mut fields) if fields.len() == 1 => {
                        match fields.remove(0) {
                            golem_wasm::Value::Record(mut fields) if fields.len() == 2 => {
                                let hi = u64::from_value(fields.remove(0))?;
                                let lo = u64::from_value(fields.remove(0))?;
                                Ok($name(uuid::Uuid::from_u64_pair(hi, lo)))
                            }
                            other => Err(format!("Expected a record with two u64 fields, got {other:?}"))
                        }
                    }
                    other => Err(format!("Expected a record with one record field, got {other:?}"))
                }
            }
        }

        $(
            #[cfg(feature = "full")]
            impl TryFrom<$proto_type> for $name {
                type Error = String;

                fn try_from(value: $proto_type) -> Result<Self, Self::Error> {
                    Ok(Self(
                        value
                            .value
                            .ok_or(format!("Missing value in {}", stringify!($name)))?
                            .into(),
                    ))
                }
            }

            #[cfg(feature = "full")]
            impl From<$name> for $proto_type {
                fn from(value: $name) -> Self {
                    $proto_type {
                        value: Some(value.0.into()),
                    }
                }
            }
        )?
    };
}

#[macro_export]
macro_rules! declare_revision {
    ($name:ident) => {
        #[derive(
            Debug,
            Copy,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            ::serde::Serialize,
            ::golem_wasm_derive::IntoValue,
            ::derive_more::Display,
        )]
        #[cfg_attr(feature = "full", derive(poem_openapi::NewType))]
        #[repr(transparent)]
        pub struct $name(pub(crate) u64);

        impl $name {
            pub const INITIAL: Self = Self(0);

            pub fn get(self) -> u64 {
                self.0
            }

            /// Returns the next revision, failing if it would exceed i64::MAX
            pub fn next(self) -> ::anyhow::Result<Self> {
                if self.0 == i64::MAX as u64 {
                    Err(::anyhow::anyhow!(
                        "Cannot increment {} beyond i64::MAX ({})",
                        stringify!($name),
                        i64::MAX
                    ))
                } else {
                    Ok(Self(self.0 + 1))
                }
            }

            pub fn new(value: u64) -> anyhow::Result<Self> {
                if value > i64::MAX as u64 {
                    Err(::anyhow::anyhow!(
                        "{} value {} exceeds i64::MAX",
                        stringify!($name),
                        value
                    ))
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl TryFrom<i64> for $name {
            type Error = ::anyhow::Error;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                if value < 0 {
                    Err(::anyhow::anyhow!(
                        "{} cannot be negative, got {}",
                        stringify!($name),
                        value
                    ))
                } else {
                    Ok(Self(value as u64))
                }
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> i64 {
                value.0 as i64
            }
        }

        impl TryFrom<u64> for $name {
            type Error = String;

            fn try_from(value: u64) -> Result<Self, Self::Error> {
                if value > i64::MAX as u64 {
                    Err(format!(
                        "{} value {} exceeds i64::MAX",
                        stringify!($name),
                        value
                    ))
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl From<$name> for u64 {
            fn from(value: $name) -> u64 {
                value.0
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = ::anyhow::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let raw: i64 = s.parse()?;
                $name::try_from(raw)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: ::serde::de::Deserializer<'de>,
            {
                let value = i64::deserialize(deserializer)?;
                $name::try_from(value).map_err(serde::de::Error::custom)
            }
        }

        impl ::golem_wasm::FromValue for $name {
            fn from_value(value: ::golem_wasm::Value) -> Result<Self, String> {
                match value {
                    ::golem_wasm::Value::U64(inner) => $name::try_from(inner),
                    _ => Err(format!(
                        "Unexpected value for {}: {value:?}",
                        stringify!($name)
                    )),
                }
            }
        }

        #[cfg(feature = "full")]
        impl ::desert_rust::BinarySerializer for $name {
            fn serialize<Output: ::desert_rust::BinaryOutput>(
                &self,
                context: &mut ::desert_rust::SerializationContext<Output>,
            ) -> ::desert_rust::Result<()> {
                ::desert_rust::BinarySerializer::serialize(&self.0, context)
            }
        }

        #[cfg(feature = "full")]
        impl ::desert_rust::BinaryDeserializer for $name {
            fn deserialize<'a, 'b>(
                context: &'a mut ::desert_rust::DeserializationContext<'b>,
            ) -> ::desert_rust::Result<Self> {
                let inner: u64 = ::desert_rust::BinaryDeserializer::deserialize(context)?;
                $name::try_from(inner).map_err(::desert_rust::Error::DeserializationFailure)
            }
        }
    };
}

#[macro_export]
macro_rules! declare_structs {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "full", derive(poem_openapi::Object))]
        #[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
        #[serde(rename_all = "camelCase")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_unions {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "full", derive(poem_openapi::Union))]
        #[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
        #[serde(tag = "type")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_enums {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
        #[cfg_attr(feature = "full", oai(rename_all = "kebab-case"))]
        #[serde(rename_all = "kebab-case")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_transparent_newtypes {
    ($($i:item)*) => { $(
    #[derive(Debug, Clone, PartialEq, ::serde::Deserialize, ::serde::Serialize)]
    #[cfg_attr(feature = "full", derive(::poem_openapi::NewType))]
    #[repr(transparent)]
        $i
    )* }
}

#[macro_export]
macro_rules! error_forwarding {
    ($target:ty $(, $sub:ty )* $(,)?) => {
        impl $crate::IntoAnyhow for $target {
            fn into_anyhow(self) -> ::anyhow::Error {
                // Note converting self into an anyhow::Error (via std::error::Error) loses the backtrace.
                // Explicitly unwrap the inner error to preserve it.
                let converted = match self {
                    Self::InternalError(inner) => inner,
                    _ => ::anyhow::Error::from(self),
                };
                converted.context(stringify!($target))
            }
        }

        $(
            impl From<$sub> for $target {
                fn from(value: $sub) -> Self {
                    // No need for special handling of anyhow::Error once backtraces are preserved when going to std::err::Error
                    // https://github.com/rust-lang/rust/issues/99301
                    Self::InternalError(<$sub as $crate::IntoAnyhow>::into_anyhow(value))
                }
            }
        )*
    };
}

/// Create a DataValue::Tuple from component model values.
///
/// Each argument is converted to ValueAndType via `.convert_to_value_and_type()` and
/// wrapped in ElementValue::ComponentModel. This is useful for creating
/// agent parameter tuples.
///
/// Values that are already `ValueAndType` are passed through unchanged (via the
/// inherent method), while other types are converted using `IntoValueAndType`.
///
/// # Example
/// ```ignore
/// let value = data_value!(42_i32, "hello", 3.14_f64);
/// // Creates DataValue::Tuple with three ElementValue::ComponentModel elements
/// ```
#[macro_export]
macro_rules! data_value {
    ($($element:expr),* $(,)?) => {
        {
            #[allow(unused_imports)]
            use golem_wasm::ConvertToValueAndType as _;
            $crate::model::agent::DataValue::Tuple(
                $crate::model::agent::ElementValues {
                    elements: vec![
                        $($crate::model::agent::ElementValue::ComponentModel(
                            $crate::model::agent::ComponentModelElementValue {
                                value: $element.convert_to_value_and_type()
                            }
                        )),*
                    ],
                }
            )
        }
    };
}

/// Create an AgentId with the given agent type name and parameters.
///
/// The first argument is the agent type name (as a string expression), followed by
/// optional component model values. Expands to:
/// `AgentId::new(AgentTypeName("NAME".to_string()), data_value!(...params), None)`
///
/// # Example
/// ```ignore
/// let id = agent_id!("my-agent");
/// let id = agent_id!("my-agent", 42u32, "hello");
/// ```
#[macro_export]
macro_rules! agent_id {
    ($name:expr) => {
        $crate::base_model::agent::AgentId::new(
            $crate::base_model::agent::AgentTypeName($name.to_string()),
            $crate::data_value!(),
            None
        )
    };
    ($name:expr, $($element:expr),+ $(,)?) => {
        $crate::base_model::agent::AgentId::new(
            $crate::base_model::agent::AgentTypeName($name.to_string()),
            $crate::data_value!($($element),+),
            None
        )
    };
}

/// Create an AgentId with a phantom UUID.
///
/// The first argument is the agent type name (as a string expression), the second is
/// a UUID, followed by optional component model values. Expands to:
/// `AgentId::new(AgentTypeName("NAME".to_string()), data_value!(...params), Some(uuid))`
///
/// # Example
/// ```ignore
/// let uuid = Uuid::now_v7();
/// let id = phantom_agent_id!("my-agent", uuid);
/// let id = phantom_agent_id!("my-agent", uuid, 42u32, "hello");
/// ```
#[macro_export]
macro_rules! phantom_agent_id {
    ($name:expr, $phantom_id:expr) => {
        $crate::base_model::agent::AgentId::new(
            $crate::base_model::agent::AgentTypeName($name.to_string()),
            $crate::data_value!(),
            Some($phantom_id)
        )
    };
    ($name:expr, $phantom_id:expr, $($element:expr),+ $(,)?) => {
        $crate::base_model::agent::AgentId::new(
            $crate::base_model::agent::AgentTypeName($name.to_string()),
            $crate::data_value!($($element),+),
            Some($phantom_id)
        )
    };
}
