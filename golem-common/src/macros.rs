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
    ($name:ident $(, $proto_type:path)?) => {
        #[derive(Clone, Debug, PartialOrd, Ord, derive_more::FromStr, Eq, Hash, PartialEq)]
        #[derive(serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            pub fn new_v4() -> $name {
                Self(uuid::Uuid::new_v4())
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

        impl bincode::Encode for $name {
            fn encode<E: bincode::enc::Encoder>(
                &self,
                encoder: &mut E,
            ) -> Result<(), bincode::error::EncodeError> {
                use bincode::enc::write::Writer;

                encoder.writer().write(self.0.as_bytes())
            }
        }

        impl<Context> bincode::Decode<Context> for $name {
            fn decode<D: bincode::de::Decoder<Context = Context>>(
                decoder: &mut D,
            ) -> Result<Self, bincode::error::DecodeError> {
                use bincode::de::read::Reader;

                let mut bytes = [0u8; 16];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(uuid::Uuid::from_bytes(bytes)))
            }
        }

        impl<'de, Context> bincode::BorrowDecode<'de, Context> for $name {
            fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = Context>>(
                decoder: &mut D,
            ) -> Result<Self, bincode::error::DecodeError> {
                use bincode::de::read::Reader;

                let mut bytes = [0u8; 16];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(uuid::Uuid::from_bytes(bytes)))
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

        impl poem_openapi::types::ParseFromParameter for $name {
            fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
                Ok(Self(value.try_into()?))
            }
        }

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

         impl ::poem_openapi::types::ParseFromMultipartField for $name {
            async fn parse_from_multipart(
                field: Option<::poem::web::Field>,
            ) -> ::poem_openapi::types::ParseResult<Self> {
                <::uuid::Uuid as poem_openapi::types::ParseFromMultipartField>::parse_from_multipart(field).await.map(Self).map_err(|e| e.propagate())
            }
        }

        impl poem_openapi::types::ToJSON for $name {
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::Value::String(self.0.to_string()))
            }
        }

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
                golem_wasm::analysis::analysed_type::record(vec![
                    golem_wasm::analysis::analysed_type::field(
                        "uuid",
                        golem_wasm::analysis::analysed_type::record(vec![
                            golem_wasm::analysis::analysed_type::field(
                                "high-bits",
                                golem_wasm::analysis::analysed_type::u64(),
                            ),
                            golem_wasm::analysis::analysed_type::field(
                                "low-bits",
                                golem_wasm::analysis::analysed_type::u64(),
                            ),
                        ]),
                    ),
                ])
            }
        }

        $(
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
            ::serde::Deserialize,
            ::serde::Serialize,
            ::bincode::Encode,
            ::bincode::Decode,
            ::golem_wasm_derive::IntoValue,
            ::derive_more::Display,
            ::derive_more::FromStr,
            poem_openapi::NewType,
        )]
        #[repr(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const INITIAL: Self = Self(0);

            pub fn next(self) -> anyhow::Result<Self> {
                Ok(Self(self.0.checked_add(1).ok_or(::anyhow::anyhow!(
                    "Failed to calculate next {}",
                    stringify!($name)
                ))?))
            }
        }

        impl From<i64> for $name {
            fn from(value: i64) -> Self {
                Self(value as u64)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> i64 {
                value.0 as i64
            }
        }
    };
}

#[macro_export]
macro_rules! declare_structs {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[derive(poem_openapi::Object)]
        #[oai(rename_all = "camelCase")]
        #[serde(rename_all = "camelCase")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_unions {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[derive(poem_openapi::Union)]
        #[oai(discriminator_name = "type", one_of = true)]
        #[serde(tag = "type")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_enums {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[derive(poem_openapi::Enum)]
        #[oai(rename_all = "kebab-case")]
        #[serde(rename_all = "kebab-case")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_transparent_newtypes {
    ($($i:item)*) => { $(
    #[derive(Debug, Clone, PartialEq, ::serde::Deserialize, ::serde::Serialize)]
    #[derive(::poem_openapi::NewType)]
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
