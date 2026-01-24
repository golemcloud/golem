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

use crate::bridge_gen::type_naming::analyzed_type_ext::AnalysedTypeExt;
use crate::bridge_gen::type_naming::TypeName;
use golem_wasm::analysis::AnalysedType;
use heck::ToUpperCamelCase;
use itertools::Itertools;
use std::fmt::{Debug, Display, Formatter};
use std::hash::Hash;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd)]
pub struct TypeScriptTypeName(String);

impl From<String> for TypeScriptTypeName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for TypeScriptTypeName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl Display for TypeScriptTypeName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TypeName for TypeScriptTypeName {
    fn from_analysed_type(_typ: &AnalysedType) -> Option<Self> {
        None
    }

    fn from_owner_and_name(owner: Option<impl AsRef<str>>, name: impl AsRef<str>) -> Self {
        match owner {
            Some(owner) => format!(
                "{}{}",
                owner.as_ref().to_upper_camel_case(),
                name.as_ref().to_upper_camel_case()
            )
            .into(),
            None => name.as_ref().to_upper_camel_case().into(),
        }
    }

    fn from_segments(segments: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        segments
            .into_iter()
            .map(|segment| segment.as_ref().to_upper_camel_case())
            .join("")
            .into()
    }

    fn requires_type_name(typ: &AnalysedType) -> bool {
        match typ {
            AnalysedType::Variant(variant) => {
                variant.cases.iter().any(|case| case.typ.can_be_named())
            }
            AnalysedType::Flags(_) | AnalysedType::Record(_) => true,
            AnalysedType::Enum(_)
            | AnalysedType::Result(_)
            | AnalysedType::Option(_)
            | AnalysedType::List(_)
            | AnalysedType::Tuple(_)
            | AnalysedType::Str(_)
            | AnalysedType::Chr(_)
            | AnalysedType::F64(_)
            | AnalysedType::F32(_)
            | AnalysedType::U64(_)
            | AnalysedType::S64(_)
            | AnalysedType::U32(_)
            | AnalysedType::S32(_)
            | AnalysedType::U16(_)
            | AnalysedType::S16(_)
            | AnalysedType::U8(_)
            | AnalysedType::S8(_)
            | AnalysedType::Bool(_)
            | AnalysedType::Handle(_) => false,
        }
    }
}

impl TypeScriptTypeName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
