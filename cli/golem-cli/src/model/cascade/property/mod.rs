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

pub mod json;
pub mod map;
pub mod optional;
pub mod tool_bindings;
pub mod vec;

#[cfg(test)]
pub(crate) mod test_support {
    use golem_common::cascade::layer::Layer;

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
