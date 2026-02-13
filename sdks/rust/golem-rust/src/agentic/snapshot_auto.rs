// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

const SNAPSHOT_VERSION: u64 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotEnvelope<T> {
    version: u64,
    state: T,
}

/// Wrapper for save dispatch using the autoref specialization trick.
///
/// When `T: Serialize`, `SaveHelper<T>` has an inherent `snapshot_save` method (preferred
/// by method resolution). For all `T`, the `SnapshotSaveFallback` trait provides a fallback
/// `snapshot_save` that is only used when the inherent method doesn't exist.
pub struct SaveHelper<'a, T>(pub &'a T);

/// Wrapper for load dispatch using the autoref specialization trick.
pub struct LoadHelper<'a, T>(pub &'a mut T);

// -- Save --

pub trait SnapshotSaveFallback {
    fn snapshot_save(&self) -> Result<super::SnapshotData, String>;
}

// Fallback: trait impl on SaveHelper<T> — lower priority than inherent methods
impl<T> SnapshotSaveFallback for SaveHelper<'_, T> {
    fn snapshot_save(&self) -> Result<super::SnapshotData, String> {
        Err("snapshot not implemented: agent type does not implement serde::Serialize + serde::de::DeserializeOwned".to_string())
    }
}

// Serde path: inherent method on SaveHelper<T> when T: Serialize — higher priority
impl<T: serde::Serialize> SaveHelper<'_, T> {
    pub fn snapshot_save(&self) -> Result<super::SnapshotData, String> {
        let envelope = SnapshotEnvelope {
            version: SNAPSHOT_VERSION,
            state: self.0,
        };
        let data = serde_json::to_vec(&envelope)
            .map_err(|e| format!("Failed to serialize agent snapshot: {}", e))?;
        Ok(super::SnapshotData {
            data,
            mime_type: "application/json".to_string(),
        })
    }
}

// -- Load --

pub trait SnapshotLoadFallback {
    fn snapshot_load(&mut self, bytes: &[u8]) -> Result<(), String>;
}

// Fallback: trait impl on LoadHelper<T> — lower priority than inherent methods
impl<T> SnapshotLoadFallback for LoadHelper<'_, T> {
    fn snapshot_load(&mut self, _bytes: &[u8]) -> Result<(), String> {
        Err("snapshot not implemented: agent type does not implement serde::Serialize + serde::de::DeserializeOwned".to_string())
    }
}

// Serde path: inherent method on LoadHelper<T> when T: DeserializeOwned — higher priority
impl<T: serde::de::DeserializeOwned> LoadHelper<'_, T> {
    pub fn snapshot_load(&mut self, bytes: &[u8]) -> Result<(), String> {
        let envelope: SnapshotEnvelope<T> = serde_json::from_slice(bytes)
            .map_err(|e| format!("Failed to deserialize agent snapshot: {}", e))?;
        if envelope.version != SNAPSHOT_VERSION {
            return Err(format!(
                "Unsupported snapshot version: {}, expected {}",
                envelope.version, SNAPSHOT_VERSION
            ));
        }
        *self.0 = envelope.state;
        Ok(())
    }
}
