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

use crate::model::cascade::layer::Layer;

pub mod json;
pub mod map;
pub mod optional;
pub mod vec;

pub trait Property<L: Layer> {
    type Value;
    type PropertyLayer;
    type TraceElem;

    fn value(&self) -> &Self::Value;
    fn trace(&self) -> &[Self::TraceElem];

    fn apply_layer(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        layer: Self::PropertyLayer,
    );

    fn compact_trace(&mut self);
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::model::cascade::layer::Layer;

    #[derive(Debug, Clone, serde::Serialize)]
    pub struct TestLayer {
        pub id: String,
    }

    impl Layer for TestLayer {
        type Id = String;
        type Value = ();
        type Selector = ();
        type AppliedSelection = String;
        type ApplyContext = ();
        type ApplyError = ();

        fn id(&self) -> &Self::Id {
            &self.id
        }

        fn parent_layers(&self) -> &[Self::Id] {
            &[]
        }

        fn apply_onto_parent(
            &self,
            _ctx: &Self::ApplyContext,
            _selector: &Self::Selector,
            _value: &mut Self::Value,
        ) -> Result<(), Self::ApplyError> {
            Ok(())
        }
    }
}
