// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
macro_rules! oplog_entry {
    (
        $($(#[$casemeta:meta])* $case:ident {
          hint: $hint:literal
          wit_raw_type: $wit_raw_type:literal
          wit_public_type: $wit_public_type:literal
          raw {
            $($(#[$meta:meta])* $field:ident: $typ:ty),* $(,)?
          }
          public {
            $($(#[$pubmeta:meta])* $pubfield:ident: $pubtyp:ty),* $(,)?
          }
      }),* $(,)?
    ) => {
        #[cfg(feature = "full")]
        #[derive(Clone, Debug, PartialEq, desert_rust::BinaryCodec)]
        pub enum OplogEntry {
            $($(#[$casemeta])*    $case {
                /// Timestamp of when the oplog entry has been created
                timestamp: Timestamp,
                $($(#[$meta])* $field: $typ),*
            }),*
        }

        #[cfg(feature = "full")]
        impl OplogEntry {
            $(ident_mash::mash! {
                constructor_name = :snake_case($case) =>

                pub fn $constructor_name($( $field: $typ ),*) -> Self {
                    Self::$case {
                        timestamp: Timestamp::now_utc(),
                        $( $field ),*
                    }
                }
            })*

            pub fn timestamp(&self) -> Timestamp {
                match self {
                    $(Self::$case { timestamp, .. } => *timestamp),*
                }
            }

            /// True if the oplog entry is a "hint" that should be skipped during replay
            pub fn is_hint(&self) -> bool {
                match self {
                    $(Self::$case { .. } => $hint),*
                }
            }

            pub fn rounded(self) -> Self {
                 match self {
                    $(Self::$case { timestamp, $( $field ),* } => Self::$case { timestamp: timestamp.rounded(), $( $field ),* }),*
                 }
            }
        }

        pub mod public_oplog_entry {
            pub use super::*;

            $(ident_mash::mash! {
                params_name = $case + Params =>

                #[derive(Clone, Debug, serde::Serialize, PartialEq, serde::Deserialize)]
                #[cfg_attr(feature = "full", derive(poem_openapi::Object))]
                #[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
                #[serde(rename_all = "camelCase")]
                pub struct $params_name {
                    pub timestamp: Timestamp,
                    $($(#[$pubmeta])* pub $pubfield: $pubtyp),*
                }
            })*
        }

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "full", derive(poem_openapi::Union))]
        #[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
        #[serde(tag = "type")]
        #[allow(clippy::large_enum_variant)]
        pub enum PublicOplogEntry {
            $($case(
                ident_mash::mash! {
                    params_name = $case + Params =>

                    public_oplog_entry::$params_name
                }
            )),*
        }
    }
}

#[macro_export]
macro_rules! oplog_payload {
        ( $typename:ident => {
                $($(#[$casemeta:meta])* $case:ident {
                    $($(#[$meta:meta])* $field:ident: $typ:ty),* $(,)?
                }),* $(,)?
        }) => {

        #[derive(Clone, Debug, PartialEq, desert_rust::BinaryCodec)]
        #[allow(clippy::large_enum_variant)]
        pub enum $typename {
            Custom($crate::schema::TypedSchemaValue),
            $($(#[$casemeta])* $case(
                ident_mash::mash! {
                    inner_name = $typename + $case => $inner_name
                }
            )),*,
        }

        impl $typename {
            /// Schema-native counterpart of the legacy `into_value_and_type`:
            /// renders the payload to a self-contained `TypedSchemaValue`.
            pub fn into_typed_schema_value(
                self,
            ) -> Result<$crate::schema::TypedSchemaValue, $crate::schema::validation::SchemaError> {
                #[allow(unused_imports)]
                use $crate::schema::IntoTypedSchemaValue as _;
                match self {
                    $(Self::$case(value) => value.into_typed_schema_value()),*,
                    Self::Custom(typed) => Ok(typed),
                }
            }
        }

        impl From<$crate::schema::TypedSchemaValue> for $typename {
            fn from(typed: $crate::schema::TypedSchemaValue) -> Self {
                Self::Custom(typed)
            }
        }

        impl TryFrom<$typename> for $crate::schema::TypedSchemaValue {
            type Error = String;

            fn try_from(value: $typename) -> Result<Self, Self::Error> {
                match value {
                    $typename::Custom(value) => Ok(value),
                    other => Err(format!("Expected Custom, got {other:?}")),
                }
            }
        }

        $(ident_mash::mash! {
            inner_name = $typename + $case =>

            #[derive(Clone, Debug, PartialEq, desert_rust::BinaryCodec, golem_schema_derive::IntoSchema, golem_schema_derive::FromSchema)]
            pub struct $inner_name {
                $( $(#[$meta])* pub $field: $typ ),*
            }

            impl From<$inner_name> for $typename {
                fn from(value: $inner_name) -> Self {
                    Self::$case(value)
                }
            }

            impl TryFrom<$typename> for $inner_name {
                type Error = String;

                fn try_from(value: $typename) -> Result<Self, Self::Error> {
                    match value {
                        $typename::$case(value) => Ok(value),
                        other => Err(format!("Expected {}, got {other:?}", stringify!($case))),
                    }
                }
            }
        })*
    }
}

#[macro_export]
macro_rules! host_payload_pairs {
    ( $( ($typename:ident => $iface:literal, $func:literal, $reqtype:ident, $resptype:ident) ),* $(,)? ) => {
        $(
            ident_mash::mash! {
                inner_req = "HostRequest" + $reqtype &
                inner_resp = "HostResponse" + $resptype =>
                pub struct $typename;

                impl $crate::model::oplog::payload::HostPayloadPair for $typename {
                    type Req = $crate::model::oplog::payload::$inner_req;
                    type Resp = $crate::model::oplog::payload::$inner_resp;

                    const INTERFACE: &'static str = $iface;
                    const FUNCTION: &'static str = $func;
                    const FQFN: &'static str = concat!($iface, "::", $func);

                    const HOST_FUNCTION_NAME: HostFunctionName = HostFunctionName::$typename;
                }
            }
        )*

        pub fn host_request_from_typed_schema_value(fqfn: &str, typed_schema_value: $crate::schema::TypedSchemaValue) -> Result<$crate::model::oplog::payload::HostRequest, String> {
            match fqfn {
                $(
                    concat!($iface, "::", $func) =>
                        ident_mash::mash! {
                            inner_req = "HostRequest" + $reqtype =>
                            Ok($crate::model::oplog::payload::HostRequest::$reqtype(<$crate::model::oplog::payload::$inner_req as $crate::schema::FromSchema>::from_value(typed_schema_value.value()).map_err(|e| e.to_string())?))
                        }
                ),*,
                _ => Ok($crate::model::oplog::payload::HostRequest::Custom(typed_schema_value))
            }
        }

        pub fn host_response_from_typed_schema_value(fqfn: &str, typed_schema_value: $crate::schema::TypedSchemaValue) -> Result<$crate::model::oplog::payload::HostResponse, String> {
            match fqfn {
                $(
                    concat!($iface, "::", $func) =>
                        ident_mash::mash! {
                            inner_resp = "HostResponse" + $resptype =>
                            Ok($crate::model::oplog::payload::HostResponse::$resptype(<$crate::model::oplog::payload::$inner_resp as $crate::schema::FromSchema>::from_value(typed_schema_value.value()).map_err(|e| e.to_string())?))
                        }
                ),*,
                _ => Ok($crate::model::oplog::payload::HostResponse::Custom(typed_schema_value))
            }
        }

        #[derive(Debug, Clone, PartialEq, desert_rust::BinaryCodec)]
        pub enum HostFunctionName {
            Custom(String),
            $(
                $typename
            ),*
        }

        impl From<&str> for HostFunctionName {
            fn from(s: &str) -> Self {
                match s {
                    $(
                        concat!($iface, "::", $func) => HostFunctionName::$typename
                    ),*,
                    _ => HostFunctionName::Custom(s.to_string())
                }
            }
        }

        impl std::fmt::Display for HostFunctionName {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    HostFunctionName::Custom(name) => write!(f, "{}", name),
                    $(
                        HostFunctionName::$typename => write!(f, "{}", <$typename as $crate::model::oplog::payload::HostPayloadPair>::FQFN)
                    ),*
                }
            }
        }
    }
}
