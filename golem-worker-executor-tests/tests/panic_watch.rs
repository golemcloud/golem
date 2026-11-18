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

use std::sync::Mutex;

// The executor is built with `panic = "abort"` (`[profile.release]` and `[profile.dev]` in the
// root `Cargo.toml`), so a panic that the unwinding test harness contains in a spawned task
// aborts a production executor. Panics that name one of the watched agents, and every state
// actor panic, are recorded.
static WATCHED_PANICS: std::sync::OnceLock<Mutex<(Vec<String>, Vec<String>)>> =
    std::sync::OnceLock::new();

// Matched as well as the agent name, so a change in how the agent is named in the panic message
// cannot make the check pass without looking.
const STATE_ACTOR_PANIC: &str = "Worker state actor";

pub fn watch_panics_naming(name: &str) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    let watched = WATCHED_PANICS.get_or_init(|| Mutex::new((Vec::new(), Vec::new())));
    watched.lock().unwrap().0.push(name.to_string());
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let message = info.to_string();
            if let Some(watched) = WATCHED_PANICS.get()
                && let Ok(mut watched) = watched.lock()
                && (message.contains(STATE_ACTOR_PANIC)
                    || watched.0.iter().any(|name| message.contains(name.as_str())))
            {
                watched.1.push(message);
            }
            previous(info);
        }));
    });
}

pub fn panics_naming(name: &str) -> Vec<String> {
    WATCHED_PANICS
        .get()
        .map(|watched| {
            watched
                .lock()
                .unwrap()
                .1
                .iter()
                .filter(|message| message.contains(name) || message.contains(STATE_ACTOR_PANIC))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
