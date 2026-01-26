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

use golem_wasm::analysis::AnalysedType;

fn is_path_leaf_type(typ: &AnalysedType) -> bool {
    match typ {
        AnalysedType::Variant(_)
        | AnalysedType::Result(_)
        | AnalysedType::Option(_)
        | AnalysedType::Record(_)
        | AnalysedType::Tuple(_)
        | AnalysedType::List(_) => false,
        AnalysedType::Enum(_)
        | AnalysedType::Flags(_)
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
        | AnalysedType::Handle(_) => true,
    }
}

fn can_be_named(typ: &AnalysedType) -> bool {
    match typ {
        AnalysedType::Variant(_)
        | AnalysedType::Result(_)
        | AnalysedType::Option(_)
        | AnalysedType::Enum(_)
        | AnalysedType::Flags(_)
        | AnalysedType::Record(_)
        | AnalysedType::Tuple(_)
        | AnalysedType::List(_)
        | AnalysedType::Handle(_) => true,
        AnalysedType::Str(_)
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
        | AnalysedType::Bool(_) => false,
    }
}

pub trait AnalysedTypeExt {
    fn is_path_leaf_type(&self) -> bool;

    fn as_path_elem_type(&self) -> Option<&AnalysedType>;

    fn can_be_named(&self) -> bool;
}

impl AnalysedTypeExt for AnalysedType {
    fn is_path_leaf_type(&self) -> bool {
        is_path_leaf_type(self)
    }

    fn as_path_elem_type(&self) -> Option<&AnalysedType> {
        (!self.is_path_leaf_type()).then_some(self)
    }

    fn can_be_named(&self) -> bool {
        can_be_named(self)
    }
}

impl AnalysedTypeExt for Option<AnalysedType> {
    fn is_path_leaf_type(&self) -> bool {
        self.as_ref().is_none_or(AnalysedType::is_path_leaf_type)
    }

    fn as_path_elem_type(&self) -> Option<&AnalysedType> {
        self.as_ref().and_then(AnalysedType::as_path_elem_type)
    }

    fn can_be_named(&self) -> bool {
        self.as_ref().is_some_and(AnalysedType::can_be_named)
    }
}

impl AnalysedTypeExt for Option<Box<AnalysedType>> {
    fn is_path_leaf_type(&self) -> bool {
        self.as_ref().is_none_or(|typ| typ.is_path_leaf_type())
    }

    fn as_path_elem_type(&self) -> Option<&AnalysedType> {
        self.as_ref().and_then(|typ| typ.as_path_elem_type())
    }

    fn can_be_named(&self) -> bool {
        self.as_ref().is_some_and(|typ| typ.can_be_named())
    }
}
