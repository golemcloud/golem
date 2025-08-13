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
macro_rules! declare_structs {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
        #[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
        #[serde(rename_all = "camelCase")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_unions {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
        #[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
        #[serde(tag = "type")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_enums {
    ($($i:item)*) => { $(
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
        #[cfg_attr(feature = "poem", oai(rename_all = "kebab-case"))]
        #[serde(rename_all = "kebab-case")]
        $i
    )* }
}

#[macro_export]
macro_rules! declare_transparent_newtypes {
    ($($i:item)*) => { $(
    #[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize, derive_more::Display)]
    #[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
    #[repr(transparent)]
        $i
    )* }
}
