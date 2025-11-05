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
macro_rules! oplog_entry {
    (
        $($(#[$casemeta:meta])* $case:ident {
          hint: $hint:literal
          raw {
            $($(#[$meta:meta])* $field:ident: $typ:ty),* $(,)?
          }
          public {
            $($(#[$pubmeta:meta])* $pubfield:ident: $pubtyp:ty),* $(,)?
          }
      }),* $(,)?
    ) => {
        #[derive(Clone, Debug, PartialEq, desert_rust::BinaryCodec)]
        pub enum OplogEntry {
            $($(#[$casemeta])*    $case {
                /// Timestamp of when the oplog entry has been created
                timestamp: Timestamp,
                $($(#[$meta])* $field: $typ),*
            }),*
        }

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

                #[derive(Clone, Debug, serde::Serialize, PartialEq, serde::Deserialize, golem_wasm::derive::IntoValue, poem_openapi::Object)]
                #[oai(rename_all = "camelCase")]
                #[serde(rename_all = "camelCase")]
                pub struct $params_name {
                    pub timestamp: Timestamp,
                    $($(#[$pubmeta])* pub $pubfield: $pubtyp),*
                }
            })*
        }

        #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, golem_wasm::derive::IntoValue, poem_openapi::Union)]
        #[oai(discriminator_name = "type", one_of = true)]
        #[serde(tag = "type")]
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
