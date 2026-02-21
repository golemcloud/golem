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

#[derive(Debug, thiserror::Error)]
pub enum StoreGetValueError<L: Layer> {
    #[error("requested layer not found: {0}")]
    LayerNotFound(L::Id),
    #[error("layer ({0}) apply error: {1}")]
    LayerApplyError(L::Id, L::ApplyError),
}

#[derive(Debug, thiserror::Error)]
pub enum StoreAddLayerError<L: Layer> {
    #[error("layer already exists: {0}")]
    LayerAlreadyExists(L::Id),
}
