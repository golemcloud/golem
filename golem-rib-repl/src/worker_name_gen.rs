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

use std::collections::HashMap;
use std::sync::RwLock;

// When it comes to REPL, unlike the regular Rib execution,
// it recompiles from the start anytime to figure out the types
// however it shouldn't result in a variable having a different instance of worker,
// meaning different worker name. Rib internally generates a worker name at compile time
// for instances without worker-name, i.e, `instance()` compared to `instance("my-worker")`.
struct ReplWorkerNameGen {
    pub instance_count: RwLock<u64>,
    pub worker_name_cache: RwLock<HashMap<u64, String>>,
}

impl ReplWorkerNameGen {
    pub fn new() -> Self {
        ReplWorkerNameGen {
            instance_count: RwLock::new(0),
            worker_name_cache: RwLock::new(HashMap::new()),
        }
    }

    // A reset prior to any compilation will only reset the instance count,
    // keeping the cache. There  is no way to reset the cache as far as the REPL session is active
    pub fn reset(&self) {
        let mut instance_count = self.instance_count.write().unwrap();
        *instance_count = 0;
    }

}

impl WorkerNameGen for ReplWorkerNameGen {
    fn generate_worker_name(&self) -> String {
        let mut instance_count = self.instance_count.write().unwrap();
        *instance_count += 1;

        let mut cache = self.worker_name_cache.write().unwrap();

        if let Some(name) = cache.get(&instance_count) {
            return name.clone();
        }
        let uuid = uuid::Uuid::new_v4();
        let name = format!("worker-{}-{}", instance_count, uuid);
        cache.insert(*instance_count, name.clone());
        name
    }
}