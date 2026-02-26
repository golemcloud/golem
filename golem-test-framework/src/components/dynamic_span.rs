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
use std::sync::{Mutex, OnceLock};
use tracing::Span;
use tracing_core::callsite::{DefaultCallsite, Identifier};
use tracing_core::field::FieldSet;
use tracing_core::metadata::Kind;
use tracing_core::{Callsite, Interest, Level, Metadata};

#[derive(Default)]
struct DynCallsite {
    delegate: OnceLock<DefaultCallsite>,
}

impl Callsite for DynCallsite {
    fn set_interest(&self, interest: Interest) {
        if let Some(d) = self.delegate.get() {
            d.set_interest(interest);
        }
    }

    fn metadata(&self) -> &Metadata<'_> {
        self.delegate
            .get()
            .expect("DynCallsite not initialized")
            .metadata()
    }
}

static INTERN_CACHE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();

fn intern(s: &str) -> &'static str {
    let mut map = INTERN_CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(leaked) = map.get(s) {
        return leaked;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    map.insert(s.to_string(), leaked);
    leaked
}

fn new_callsite_and_meta(
    name: &str,
    target: &str,
    level: Level,
    field_names: &[&str],
    kind: Kind,
    file: Option<&str>,
    line: Option<u32>,
) -> &'static Metadata<'static> {
    let static_field_names: Vec<&'static str> =
        field_names.iter().map(|n| intern(n)).collect();
    let static_field_names_slice: &'static [&'static str] =
        Box::leak(static_field_names.into_boxed_slice());

    let callsite: &'static DynCallsite = Box::leak(Box::<DynCallsite>::default());
    let meta: &'static Metadata<'static> = Box::leak(Box::new(Metadata::new(
        intern(name),
        intern(target),
        level,
        file.map(|f| intern(f)),
        line,
        None,
        FieldSet::new(static_field_names_slice, Identifier(callsite)),
        kind,
    )));

    callsite
        .delegate
        .set(DefaultCallsite::new(meta))
        .expect("DynCallsite already initialized");

    meta
}

// --------------- Span support ---------------

static SPAN_CACHE: OnceLock<Mutex<HashMap<String, &'static Metadata<'static>>>> = OnceLock::new();

fn span_cache() -> &'static Mutex<HashMap<String, &'static Metadata<'static>>> {
    SPAN_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn span_cache_key(target: &str, name: &str, field_names: &mut Vec<&str>) -> String {
    field_names.sort();
    if field_names.is_empty() {
        format!("{target}:{name}")
    } else {
        format!("{target}:{}:{}", name, field_names.join(","))
    }
}

fn get_or_create_span_meta(
    target: &str,
    name: &str,
    field_names: &[&str],
    cache_key: &str,
) -> &'static Metadata<'static> {
    let mut map = span_cache().lock().unwrap();
    if let Some(meta) = map.get(cache_key) {
        return meta;
    }

    let meta = new_callsite_and_meta(name, target, Level::TRACE, field_names, Kind::SPAN, None, None);
    map.insert(cache_key.to_string(), meta);
    meta
}

pub fn make_span(target: &str, name: &str, fields: &[(String, String)]) -> Span {
    let mut field_names: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    let cache_key = span_cache_key(target, name, &mut field_names);
    let meta = get_or_create_span_meta(target, name, &field_names, &cache_key);

    let span = Span::new(meta, &meta.fields().value_set(&[]));
    for (k, v) in fields {
        span.record(k.as_str(), v.as_str());
    }
    span
}

// --------------- Event support ---------------

static EVENT_CACHE: OnceLock<Mutex<HashMap<String, &'static Metadata<'static>>>> = OnceLock::new();

fn event_cache() -> &'static Mutex<HashMap<String, &'static Metadata<'static>>> {
    EVENT_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn level_key(level: Level) -> &'static str {
    match level {
        Level::TRACE => "T",
        Level::DEBUG => "D",
        Level::INFO => "I",
        Level::WARN => "W",
        Level::ERROR => "E",
    }
}

fn get_or_create_event_meta(target: &str, level: Level, file: Option<&str>, line: Option<u32>) -> &'static Metadata<'static> {
    let cache_key = format!("{}:{}:{}:{}", target, level_key(level), file.unwrap_or(""), line.unwrap_or(0));
    let mut map = event_cache().lock().unwrap();
    if let Some(meta) = map.get(&cache_key) {
        return meta;
    }

    let meta =
        new_callsite_and_meta("child_process_event", target, level, &["message"], Kind::EVENT, file, line);
    map.insert(cache_key, meta);
    meta
}

pub fn dispatch_event(target: &str, level: Level, message: &str, file: Option<&str>, line: Option<u32>) {
    let meta = get_or_create_event_meta(target, level, file, line);
    let message_field = meta.fields().field("message").unwrap();
    tracing_core::Event::dispatch(
        meta,
        &meta
            .fields()
            .value_set(&[(&message_field, Some(&message as &dyn tracing_core::field::Value))]),
    );
}
