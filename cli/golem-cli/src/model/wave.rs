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

use golem_wasm_ast::analysis::{
    AnalysedFunction, AnalysedType, TypeRecord, TypeResult, TypeTuple, TypeVariant,
};

pub fn type_wave_compatible(typ: &AnalysedType) -> bool {
    fn variant_wave_compatible(tv: &TypeVariant) -> bool {
        tv.cases
            .iter()
            .all(|notp| notp.typ.iter().all(type_wave_compatible))
    }

    fn result_wave_compatible(tr: &TypeResult) -> bool {
        tr.ok.iter().all(|t| type_wave_compatible(t))
            && tr.err.iter().all(|t| type_wave_compatible(t))
    }

    fn record_wave_compatible(tr: &TypeRecord) -> bool {
        tr.fields.iter().all(|ntp| type_wave_compatible(&ntp.typ))
    }

    fn tuple_wave_compatible(tt: &TypeTuple) -> bool {
        tt.items.iter().all(type_wave_compatible)
    }

    match typ {
        AnalysedType::Variant(tv) => variant_wave_compatible(tv),
        AnalysedType::Result(tr) => result_wave_compatible(tr),
        AnalysedType::Option(to) => type_wave_compatible(&to.inner),
        AnalysedType::Enum(_) => true,
        AnalysedType::Flags(_) => true,
        AnalysedType::Record(tr) => record_wave_compatible(tr),
        AnalysedType::Tuple(tt) => tuple_wave_compatible(tt),
        AnalysedType::List(tl) => type_wave_compatible(&tl.inner),
        AnalysedType::Str(_) => true,
        AnalysedType::Chr(_) => true,
        AnalysedType::F64(_) => true,
        AnalysedType::F32(_) => true,
        AnalysedType::U64(_) => true,
        AnalysedType::S64(_) => true,
        AnalysedType::U32(_) => true,
        AnalysedType::S32(_) => true,
        AnalysedType::U16(_) => true,
        AnalysedType::S16(_) => true,
        AnalysedType::U8(_) => true,
        AnalysedType::S8(_) => true,
        AnalysedType::Bool(_) => true,
        AnalysedType::Handle(_) => false,
    }
}

pub fn function_wave_compatible(func: &AnalysedFunction) -> bool {
    func.parameters.iter().all(|p| type_wave_compatible(&p.typ))
        && func.result.iter().all(|r| type_wave_compatible(&r.typ))
}
