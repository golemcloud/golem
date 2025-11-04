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
pub(crate) use crate::model::cascade::layer::Layer;
use std::cell::{Ref, RefCell};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Store<L: Layer> {
    layers: HashMap<L::Id, L>,
    value_cache: RefCell<HashMap<L::Id, HashMap<L::Selector, L::Value>>>,
}

// TODO: check for circular parents (either on add or on get, or have a separate validation step)
impl<L: Layer> Store<L> {
    pub fn new() -> Store<L> {
        Self {
            layers: HashMap::new(),
            value_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn add_layer(&mut self, layer: L) -> Result<(), StoreAddLayerError<L>> {
        if self.layers.contains_key(&layer.id()) {
            return Err(StoreAddLayerError::LayerAlreadyExists(layer.id().clone()));
        }
        self.layers.insert(layer.id().clone(), layer);
        Ok(())
    }

    pub fn value(
        &self,
        id: &L::Id,
        selector: &L::Selector,
    ) -> Result<Ref<'_, L::Value>, StoreGetValueError<L>> {
        {
            let value_cache = self.value_cache.borrow();
            if value_cache
                .get(id)
                .map(|v| v.contains_key(selector))
                .unwrap_or(false)
            {
                return Ok(Ref::map(value_cache, |value_cache| {
                    value_cache.get(id).unwrap().get(selector).unwrap()
                }));
            }
        }

        let Some(layer) = self.layers.get(id) else {
            return Err(StoreGetValueError::LayerNotFound(id.clone()));
        };

        fn apply_layer<L: Layer>(
            store: &Store<L>,
            selector: &L::Selector,
            layer: &L,
            value: &mut L::Value,
        ) -> Result<(), StoreGetValueError<L>> {
            for layer_id in layer.parent_layers() {
                let Some(layer) = store.layers.get(layer_id) else {
                    return Err(StoreGetValueError::LayerNotFound(layer_id.clone()));
                };
                apply_layer(store, selector, layer, value)?;
            }
            if let Some(err) = layer.apply_onto_parent(selector, value).err() {
                return Err(StoreGetValueError::LayerApplyError(layer.id().clone(), err));
            };
            Ok(())
        }
        let mut value = L::Value::default();
        apply_layer(self, selector, layer, &mut value)?;

        {
            let mut value_cache = self.value_cache.borrow_mut();
            let value_cache = match value_cache.get_mut(id) {
                Some(value_cache) => value_cache,
                None => {
                    value_cache.insert(id.clone(), HashMap::new());
                    value_cache.get_mut(id).unwrap()
                }
            };
            value_cache.insert(selector.clone(), value);
        }

        Ok(Ref::map(self.value_cache.borrow(), |value_cache| {
            value_cache.get(id).unwrap().get(selector).unwrap()
        }))
    }
}
