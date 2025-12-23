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
use serde_derive::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum OptionalPropertyTraceElem<L: Layer, V> {
    #[serde(rename_all = "camelCase")]
    Override {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        value: V,
    },
    #[serde(rename_all = "camelCase")]
    Skip {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionalProperty<L: Layer, V> {
    value: Option<V>,
    trace: Vec<OptionalPropertyTraceElem<L, V>>,
}

impl<L: Layer, V> Default for OptionalProperty<L, V> {
    fn default() -> Self {
        Self::none()
    }
}

impl<L: Layer, V> OptionalProperty<L, V> {
    pub fn new(value: Option<V>) -> Self {
        Self {
            value,
            trace: vec![],
        }
    }

    pub fn some(value: V) -> Self {
        Self::new(Some(value))
    }

    pub fn none() -> Self {
        Self::new(None)
    }
}

impl<L: Layer, V> From<V> for OptionalProperty<L, V> {
    fn from(value: V) -> Self {
        Self::new(Some(value))
    }
}

impl<V: Clone, L: Layer> From<&V> for OptionalProperty<L, V> {
    fn from(value: &V) -> Self {
        Self::new(Some(value.to_owned()))
    }
}

impl<L: Layer, V> From<Option<V>> for OptionalProperty<L, V> {
    fn from(value: Option<V>) -> Self {
        Self::new(value)
    }
}

impl<V: Clone, L: Layer> From<Option<&V>> for OptionalProperty<L, V> {
    fn from(value: Option<&V>) -> Self {
        value.cloned().into()
    }
}

impl<V: Clone, L: Layer> Property<L> for OptionalProperty<L, V> {
    type Value = Option<V>;
    type PropertyLayer = Option<V>;
    type TraceElem = OptionalPropertyTraceElem<L, V>;

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
        match layer {
            Some(value) => {
                self.value = Some(value.clone());
                self.trace.push(OptionalPropertyTraceElem::Override {
                    id: id.clone(),
                    selection: selection.cloned(),
                    value,
                })
            }
            None => self.trace.push(OptionalPropertyTraceElem::Skip {
                id: id.clone(),
                selection: selection.cloned(),
            }),
        }
    }

    fn compact_trace(&mut self) {
        self.trace.retain(|elem| match elem {
            OptionalPropertyTraceElem::Override { .. } => true,
            OptionalPropertyTraceElem::Skip { .. } => false,
        })
    }
}
