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

use crate::model::cascade::error::{StoreAddLayerError, StoreGetValueError};
use crate::model::cascade::layer::Layer;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Store<L: Layer> {
    layers: HashMap<L::Id, L>,
}

impl<L: Layer> Default for Store<L> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L: Layer> Store<L> {
    pub fn new() -> Store<L> {
        Self {
            layers: HashMap::new(),
        }
    }

    pub fn add_layer(&mut self, layer: L) -> Result<(), StoreAddLayerError<L>> {
        if self.layers.contains_key(layer.id()) {
            return Err(StoreAddLayerError::LayerAlreadyExists(layer.id().clone()));
        }
        self.layers.insert(layer.id().clone(), layer);
        Ok(())
    }

    pub fn value(
        &self,
        id: &L::Id,
        selector: &L::Selector,
        ctx: &L::ApplyContext,
    ) -> Result<L::Value, StoreGetValueError<L>> {
        self.value_internal(ctx, id, selector)
    }

    /// Checks that the given layers, treated as the ordered parents of a single layer, reach
    /// every ancestor through a single parent path, as [`Store::value`] requires for a layer's
    /// own parents.
    pub fn check_single_path_ancestry(&self, roots: &[L::Id]) -> Result<(), StoreGetValueError<L>> {
        let mut path = Vec::new();
        let mut visited = HashMap::new();
        for root in roots {
            let Some(layer) = self.layers.get(root) else {
                return Err(StoreGetValueError::LayerNotFound(root.clone()));
            };
            self.visit_layer(layer, &mut path, &mut visited, &mut |_| Ok(()))?;
        }
        Ok(())
    }

    fn value_internal(
        &self,
        ctx: &L::ApplyContext,
        id: &L::Id,
        selector: &L::Selector,
    ) -> Result<L::Value, StoreGetValueError<L>> {
        let Some(layer) = self.layers.get(id) else {
            return Err(StoreGetValueError::LayerNotFound(id.clone()));
        };

        let mut value = L::Value::default();
        let mut path = Vec::new();
        let mut visited = HashMap::new();
        self.visit_layer(layer, &mut path, &mut visited, &mut |layer| {
            layer
                .apply_onto_parent(ctx, selector, &mut value)
                .map_err(|err| StoreGetValueError::LayerApplyError(layer.id().clone(), err))
        })?;
        Ok(value)
    }

    // Parents are visited depth-first before their child. Every layer must be reachable through
    // a single parent path: a layer inherited through multiple paths (diamond inheritance) is
    // rejected, so the applied order is always the one written in the layer definitions.
    fn visit_layer<'a>(
        &'a self,
        layer: &'a L,
        path: &mut Vec<&'a L::Id>,
        visited: &mut HashMap<&'a L::Id, Vec<&'a L::Id>>,
        on_layer: &mut impl FnMut(&'a L) -> Result<(), StoreGetValueError<L>>,
    ) -> Result<(), StoreGetValueError<L>> {
        let layer_id = layer.id();
        let to_owned_path = |path: &[&L::Id]| {
            path.iter()
                .map(|id| (*id).clone())
                .chain(std::iter::once(layer_id.clone()))
                .collect::<Vec<_>>()
        };
        if path.contains(&layer_id) {
            return Err(StoreGetValueError::CircularParents(to_owned_path(path)));
        }
        if let Some(first_path) = visited.get(layer_id) {
            return Err(StoreGetValueError::MultipleParentPaths {
                layer: layer_id.clone(),
                first_path: first_path.iter().map(|id| (*id).clone()).collect(),
                second_path: to_owned_path(path),
            });
        }
        path.push(layer_id);
        for parent_id in layer.parent_layers() {
            let Some(parent) = self.layers.get(parent_id) else {
                return Err(StoreGetValueError::LayerNotFound(parent_id.clone()));
            };
            self.visit_layer(parent, path, visited, on_layer)?;
        }
        on_layer(layer)?;
        visited.insert(layer_id, path.clone());
        path.pop();
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::Store;
    use crate::model::cascade::error::StoreGetValueError;
    use crate::model::cascade::layer::Layer;
    use test_r::test;

    #[derive(Debug, Clone, serde::Serialize)]
    struct TestLayer {
        id: String,
        parents: Vec<String>,
    }

    impl Layer for TestLayer {
        type Id = String;
        type Value = Vec<String>;
        type Selector = ();
        type AppliedSelection = ();
        type ApplyContext = ();
        type ApplyError = ();

        fn id(&self) -> &Self::Id {
            &self.id
        }

        fn parent_layers(&self) -> &[Self::Id] {
            &self.parents
        }

        fn apply_onto_parent(
            &self,
            _ctx: &Self::ApplyContext,
            _selector: &Self::Selector,
            value: &mut Self::Value,
        ) -> Result<(), Self::ApplyError> {
            value.push(self.id.clone());
            Ok(())
        }
    }

    #[test]
    fn value_applies_parent_layers_before_target() {
        let mut store = Store::<TestLayer>::new();
        store
            .add_layer(TestLayer {
                id: "base".to_string(),
                parents: vec![],
            })
            .unwrap();
        store
            .add_layer(TestLayer {
                id: "mid".to_string(),
                parents: vec!["base".to_string()],
            })
            .unwrap();
        store
            .add_layer(TestLayer {
                id: "leaf".to_string(),
                parents: vec!["mid".to_string()],
            })
            .unwrap();

        let value = store.value(&"leaf".to_string(), &(), &()).unwrap();
        assert_eq!(value, vec!["base", "mid", "leaf"]);
    }

    fn add(store: &mut Store<TestLayer>, id: &str, parents: &[&str]) {
        store
            .add_layer(TestLayer {
                id: id.to_string(),
                parents: parents.iter().map(|p| p.to_string()).collect(),
            })
            .unwrap();
    }

    #[test]
    fn value_detects_circular_parents_instead_of_overflowing() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "a", &["b"]);
        add(&mut store, "b", &["a"]);

        let err = store.value(&"a".to_string(), &(), &()).unwrap_err();
        assert!(
            matches!(err, StoreGetValueError::CircularParents(_)),
            "expected CircularParents, got {err:?}"
        );
    }

    #[test]
    fn value_detects_self_referencing_parent() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "a", &["a"]);

        let err = store.value(&"a".to_string(), &(), &()).unwrap_err();
        assert!(matches!(err, StoreGetValueError::CircularParents(_)));
    }

    #[test]
    fn value_rejects_diamond_shaped_parents() {
        // a -> {b, c} -> d : d is reachable via two paths.
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "d", &[]);
        add(&mut store, "b", &["d"]);
        add(&mut store, "c", &["d"]);
        add(&mut store, "a", &["b", "c"]);

        match store.value(&"a".to_string(), &(), &()).unwrap_err() {
            StoreGetValueError::MultipleParentPaths {
                layer,
                first_path,
                second_path,
            } => {
                assert_eq!(layer, "d");
                assert_eq!(first_path, vec!["a", "b", "d"]);
                assert_eq!(second_path, vec!["a", "c", "d"]);
            }
            err => panic!("expected MultipleParentPaths, got {err:?}"),
        }
    }

    #[test]
    fn value_rejects_parent_listed_twice() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "b", &[]);
        add(&mut store, "a", &["b", "b"]);

        let err = store.value(&"a".to_string(), &(), &()).unwrap_err();
        assert!(
            matches!(err, StoreGetValueError::MultipleParentPaths { .. }),
            "expected MultipleParentPaths, got {err:?}"
        );
    }

    #[test]
    fn single_path_ancestry_accepts_independent_roots() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "base", &[]);
        add(&mut store, "other", &[]);
        add(&mut store, "derived", &["base"]);

        store
            .check_single_path_ancestry(&["derived".to_string(), "other".to_string()])
            .unwrap();
    }

    #[test]
    fn single_path_ancestry_rejects_roots_sharing_an_ancestor() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "base", &[]);
        add(&mut store, "derived", &["base"]);

        match store
            .check_single_path_ancestry(&["base".to_string(), "derived".to_string()])
            .unwrap_err()
        {
            StoreGetValueError::MultipleParentPaths {
                layer,
                first_path,
                second_path,
            } => {
                assert_eq!(layer, "base");
                assert_eq!(first_path, vec!["base"]);
                assert_eq!(second_path, vec!["derived", "base"]);
            }
            err => panic!("expected MultipleParentPaths, got {err:?}"),
        }
    }

    #[test]
    fn single_path_ancestry_rejects_root_listed_twice() {
        let mut store = Store::<TestLayer>::new();
        add(&mut store, "base", &[]);

        let err = store
            .check_single_path_ancestry(&["base".to_string(), "base".to_string()])
            .unwrap_err();
        assert!(
            matches!(err, StoreGetValueError::MultipleParentPaths { .. }),
            "expected MultipleParentPaths, got {err:?}"
        );
    }
}
