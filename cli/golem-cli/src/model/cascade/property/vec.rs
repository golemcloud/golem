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

use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::Property;
use serde::Serialize;
use serde_derive::Deserialize;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VecMergeMode {
    #[default]
    Append,
    Prepend,
    Replace,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum VecPropertyTraceElem<L: Layer, T> {
    #[serde(rename_all = "camelCase")]
    Append {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        appended_elems: Vec<T>,
    },
    #[serde(rename_all = "camelCase")]
    Prepend {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        prepended_elems: Vec<T>,
    },
    #[serde(rename_all = "camelCase")]
    Replace {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        new_elems: Vec<T>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VecProperty<L: Layer, T: Serialize> {
    value: Vec<T>,
    trace: Vec<VecPropertyTraceElem<L, T>>,
}

impl<L: Layer, T: Serialize> Default for VecProperty<L, T> {
    fn default() -> Self {
        Self {
            value: vec![],
            trace: vec![],
        }
    }
}

impl<L: Layer, T: Serialize> VecProperty<L, T> {
    pub fn new(vec: Vec<T>) -> Self {
        Self {
            value: vec,
            trace: vec![],
        }
    }
}

impl<L: Layer, T: Serialize> From<Vec<T>> for VecProperty<L, T> {
    fn from(value: Vec<T>) -> Self {
        Self::new(value)
    }
}

impl<L: Layer, T: Serialize + Clone> Property<L> for VecProperty<L, T> {
    type Value = Vec<T>;
    type PropertyLayer = (VecMergeMode, Vec<T>);
    type TraceElem = VecPropertyTraceElem<L, T>;

    fn value(&self) -> &Self::Value {
        &self.value
    }

    fn trace(&self) -> &[Self::TraceElem] {
        self.trace.as_slice()
    }

    fn apply_layer(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        layer: Self::PropertyLayer,
    ) {
        let (mode, elems) = layer;
        match mode {
            VecMergeMode::Append => {
                self.value.extend(elems.clone());
                self.trace.push(VecPropertyTraceElem::Append {
                    id: id.clone(),
                    selection: selection.cloned(),
                    appended_elems: elems,
                });
            }
            VecMergeMode::Prepend => {
                let mut new_vec = elems.clone();
                new_vec.extend(self.value.clone());
                self.trace.push(VecPropertyTraceElem::Prepend {
                    id: id.clone(),
                    selection: selection.cloned(),
                    prepended_elems: elems,
                });
                self.value = new_vec;
            }
            VecMergeMode::Replace => {
                self.trace.push(VecPropertyTraceElem::Replace {
                    id: id.clone(),
                    selection: selection.cloned(),
                    new_elems: elems.clone(),
                });
                self.value = elems;
            }
        }
    }

    fn compact_trace(&mut self) {
        self.trace.retain(|elem| match elem {
            VecPropertyTraceElem::Append { appended_elems, .. } => !appended_elems.is_empty(),
            VecPropertyTraceElem::Prepend {
                prepended_elems, ..
            } => !prepended_elems.is_empty(),
            VecPropertyTraceElem::Replace { .. } => true,
        })
    }
}
