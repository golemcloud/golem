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

// TODO: check for circular parents (either on add or on get, or have a separate validation step)
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
    ) -> Result<L::Value, StoreGetValueError<L>> {
        self.value_internal(&L::root_id_to_context(id), id, selector)
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

        fn apply_layer<L: Layer>(
            store: &Store<L>,
            ctx: &L::ApplyContext,
            selector: &L::Selector,
            layer: &L,
            value: &mut L::Value,
        ) -> Result<(), StoreGetValueError<L>> {
            for layer_id in layer.parent_layers() {
                let Some(layer) = store.layers.get(layer_id) else {
                    return Err(StoreGetValueError::LayerNotFound(layer_id.clone()));
                };
                apply_layer(store, ctx, selector, layer, value)?;
            }
            if let Some(err) = layer.apply_onto_parent(ctx, selector, value).err() {
                return Err(StoreGetValueError::LayerApplyError(layer.id().clone(), err));
            };
            Ok(())
        }
        let mut value = L::Value::default();
        apply_layer(self, ctx, selector, layer, &mut value)?;
        Ok(value)
    }
}
