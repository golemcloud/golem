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

use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct ParameterNaming {
    used: HashSet<String>,
    next_suffix_by_name: HashMap<String, usize>,
}

impl ParameterNaming {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve(&mut self, name: impl Into<String>) {
        self.used.insert(name.into());
    }

    pub fn reserve_many(&mut self, names: impl IntoIterator<Item = impl Into<String>>) {
        for name in names {
            self.reserve(name);
        }
    }

    pub fn fresh(&mut self, preferred_name: impl Into<String>) -> String {
        let preferred_name = preferred_name.into();

        if self.used.insert(preferred_name.clone()) {
            self.next_suffix_by_name
                .entry(preferred_name.clone())
                .or_insert(1);
            return preferred_name;
        }

        let next_suffix = self
            .next_suffix_by_name
            .entry(preferred_name.clone())
            .or_insert(1);

        loop {
            let candidate = format!("{preferred_name}{next_suffix}");
            *next_suffix += 1;

            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}
