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
        #[cfg_attr(feature = "model", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "model", serde(transparent))]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new_v4() -> $name {
                Self(Uuid::new_v4())
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
                Ok(Self(Uuid::from_bytes(bytes)))
            }
        }

        impl<'de, Context> bincode::BorrowDecode<'de, Context> for $name {
            fn borrow_decode<D: bincode::de::BorrowDecoder<'de, Context = Context>>(
                decoder: &mut D,
            ) -> Result<Self, bincode::error::DecodeError> {
                use bincode::de::read::Reader;

                let mut bytes = [0u8; 16];
                decoder.reader().read(&mut bytes)?;
                Ok(Self(Uuid::from_bytes(bytes)))
            }
        }

        impl TryFrom<&str> for $name {
            type Error = String;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                let uuid =
                    Uuid::parse_str(value).map_err(|err| format!("Invalid plan ID: {err}"))?;
                Ok(Self(uuid))
            }
        }

        #[cfg(feature = "poem")]
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

        #[cfg(feature = "poem")]
        impl poem_openapi::types::ParseFromParameter for $name {
            fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
                Ok(Self(value.try_into()?))
            }
        }

        #[cfg(feature = "poem")]
        impl poem_openapi::types::ParseFromJSON for $name {
            fn parse_from_json(
                value: Option<serde_json::Value>,
            ) -> poem_openapi::types::ParseResult<Self> {
                match value {
                    Some(serde_json::Value::String(s)) => Ok(Self(Uuid::from_str(&s)?)),
                    _ => Err(poem_openapi::types::ParseError::<$name>::custom(format!(
                        "Unexpected representation of {}",
                        stringify!($name)
                    ))),
                }
            }
        }

        #[cfg(feature = "poem")]
        impl poem_openapi::types::ToJSON for $name {
            fn to_json(&self) -> Option<serde_json::Value> {
                Some(serde_json::Value::String(self.0.to_string()))
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", &self.0)
            }
        }

        #[cfg(feature = "model")]
        impl golem_wasm_rpc::IntoValue for $name {
            fn into_value(self) -> golem_wasm_rpc::Value {
                let (hi, lo) = self.0.as_u64_pair();
                golem_wasm_rpc::Value::Record(vec![golem_wasm_rpc::Value::Record(vec![
                    golem_wasm_rpc::Value::U64(hi),
                    golem_wasm_rpc::Value::U64(lo),
                ])])
            }

            fn get_type() -> golem_wasm_ast::analysis::AnalysedType {
                golem_wasm_ast::analysis::analysed_type::record(vec![
                    golem_wasm_ast::analysis::analysed_type::field(
                        "uuid",
                        golem_wasm_ast::analysis::analysed_type::record(vec![
                            golem_wasm_ast::analysis::analysed_type::field(
                                "high-bits",
                                golem_wasm_ast::analysis::analysed_type::u64(),
                            ),
                            golem_wasm_ast::analysis::analysed_type::field(
                                "low-bits",
                                golem_wasm_ast::analysis::analysed_type::u64(),
                            ),
                        ]),
                    ),
                ])
            }
        }

        $(
            #[cfg(feature = "protobuf")]
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

            #[cfg(feature = "protobuf")]
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
