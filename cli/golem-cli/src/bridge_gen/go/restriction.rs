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

//! Schema restrictions as Go struct tags.
//!
//! The Go SDK reads a field's restrictions from a `golem:"…"` tag, on record
//! fields, method parameters and tool arguments alike. A generated struct
//! carries the tag for every restricted field, so the schema the SDK derives
//! from it matches the one it was generated from: a tool compares the two
//! exactly before it runs a command.

use crate::bridge_gen::go::go::go_string;
use golem_common::schema::schema_type::{
    NumericBound, NumericRestrictions, PathDirection, PathKind, QuantityValue, SchemaType,
};

/// The `golem:"…"` tag of a field of this type, if the type is restricted.
/// An option is restricted through to its value, a list to its elements.
pub fn restriction_tag<'a>(
    typ: &'a SchemaType,
    resolve: &impl Fn(&'a SchemaType) -> &'a SchemaType,
) -> anyhow::Result<Option<String>> {
    let mut parts: Vec<String> = Vec::new();
    let mut regex = None;
    match resolve(typ) {
        SchemaType::Option { inner, .. } => return restriction_tag(inner, resolve),
        SchemaType::List { element, .. } => return restriction_tag(element, resolve),
        SchemaType::S8 { restrictions, .. }
        | SchemaType::S16 { restrictions, .. }
        | SchemaType::S32 { restrictions, .. }
        | SchemaType::S64 { restrictions, .. }
        | SchemaType::U8 { restrictions, .. }
        | SchemaType::U16 { restrictions, .. }
        | SchemaType::U32 { restrictions, .. }
        | SchemaType::U64 { restrictions, .. }
        | SchemaType::F32 { restrictions, .. }
        | SchemaType::F64 { restrictions, .. } => {
            if let Some(NumericRestrictions { min, max, unit }) = restrictions {
                if let Some(min) = min {
                    parts.push(format!("min={}", bound(min)));
                }
                if let Some(max) = max {
                    parts.push(format!("max={}", bound(max)));
                }
                if let Some(unit) = unit {
                    parts.push(format!("unit={}", item(unit)?));
                }
            }
        }
        SchemaType::Text { restrictions, .. } => {
            if let Some(languages) = &restrictions.languages {
                parts.push(format!("languages={}", list(languages)?));
            }
            if let Some(n) = restrictions.min_length {
                parts.push(format!("minLength={n}"));
            }
            if let Some(n) = restrictions.max_length {
                parts.push(format!("maxLength={n}"));
            }
            regex = restrictions.regex.clone();
        }
        SchemaType::Binary { restrictions, .. } => {
            if let Some(mime) = &restrictions.mime_types {
                parts.push(format!("mime={}", list(mime)?));
            }
            if let Some(n) = restrictions.min_bytes {
                parts.push(format!("minBytes={n}"));
            }
            if let Some(n) = restrictions.max_bytes {
                parts.push(format!("maxBytes={n}"));
            }
        }
        SchemaType::Path { spec, .. } => {
            // The SDK's unrestricted path is in-out and of any kind.
            match spec.direction {
                PathDirection::Input => parts.push("direction=input".to_string()),
                PathDirection::Output => parts.push("direction=output".to_string()),
                PathDirection::InOut => {}
            }
            match spec.kind {
                PathKind::File => parts.push("kind=file".to_string()),
                PathKind::Directory => parts.push("kind=directory".to_string()),
                PathKind::Any => {}
            }
            if let Some(mime) = &spec.allowed_mime_types {
                parts.push(format!("mime={}", list(mime)?));
            }
            if let Some(extensions) = &spec.allowed_extensions {
                parts.push(format!("extensions={}", list(extensions)?));
            }
        }
        SchemaType::Url { restrictions, .. } => {
            if let Some(schemes) = &restrictions.allowed_schemes {
                parts.push(format!("schemes={}", list(schemes)?));
            }
            if let Some(hosts) = &restrictions.allowed_hosts {
                parts.push(format!("hosts={}", list(hosts)?));
            }
        }
        SchemaType::Quantity { spec, .. } => {
            if let Some(min) = &spec.min {
                parts.push(format!("min={}", quantity(min)?));
            }
            if let Some(max) = &spec.max {
                parts.push(format!("max={}", quantity(max)?));
            }
        }
        _ => {}
    }
    // A regex runs to the end of the tag, so it may hold commas.
    if let Some(regex) = regex {
        parts.push(format!("regex={regex}"));
    }
    if parts.is_empty() {
        return Ok(None);
    }
    let value = parts.join(",");
    if value.contains('`') {
        anyhow::bail!("the Go bridge cannot spell a restriction containing a backtick: {value}");
    }
    Ok(Some(format!("golem:{}", go_string(&value))))
}

fn bound(bound: &NumericBound) -> String {
    match bound {
        NumericBound::Signed(n) => n.to_string(),
        NumericBound::Unsigned(n) => n.to_string(),
        NumericBound::FloatBits(bits) => f64::from_bits(*bits).to_string(),
    }
}

fn quantity(value: &QuantityValue) -> anyhow::Result<String> {
    let negative = value.mantissa < 0;
    let mut digits = value.mantissa.unsigned_abs().to_string();
    if value.scale > 0 {
        let scale = value.scale as usize;
        while digits.len() <= scale {
            digits.insert(0, '0');
        }
        digits.insert(digits.len() - scale, '.');
    } else if value.scale < 0 {
        digits.push_str(&"0".repeat(value.scale.unsigned_abs() as usize));
    }
    if negative {
        digits.insert(0, '-');
    }
    Ok(format!("{digits}{}", item(&value.unit)?))
}

/// One value of a tag: it must not hold the separators the tag is split on.
fn item(value: &str) -> anyhow::Result<&str> {
    if value.contains([',', '|']) {
        anyhow::bail!(
            "the Go bridge cannot spell a restriction value containing ',' or '|': {value}"
        );
    }
    Ok(value)
}

fn list(values: &[String]) -> anyhow::Result<String> {
    Ok(values
        .iter()
        .map(|v| item(v))
        .collect::<anyhow::Result<Vec<_>>>()?
        .join("|"))
}

/// Struct field lines as gofmt writes them: names padded to one column per
/// section (a documented field starts a new one), and the types of each run of
/// tagged fields padded so their tags line up.
pub fn field_lines(
    idents: &[String],
    types: &[String],
    tags: &[Option<String>],
    documented: &[bool],
) -> Vec<String> {
    let n = idents.len();
    let mut lines = Vec::with_capacity(n);
    let mut start = 0;
    while start < n {
        let mut end = start + 1;
        while end < n && !documented[end] {
            end += 1;
        }
        let name_width = idents[start..end]
            .iter()
            .map(|i| i.len())
            .max()
            .unwrap_or(0);
        let mut i = start;
        while i < end {
            if tags[i].is_none() {
                lines.push(format!("{:<name_width$} {}", idents[i], types[i]));
                i += 1;
                continue;
            }
            let mut run_end = i;
            while run_end < end && tags[run_end].is_some() {
                run_end += 1;
            }
            let type_width = types[i..run_end].iter().map(|t| t.len()).max().unwrap_or(0);
            for j in i..run_end {
                lines.push(format!(
                    "{:<name_width$} {:<type_width$} `{}`",
                    idents[j],
                    types[j],
                    tags[j].as_ref().unwrap()
                ));
            }
            i = run_end;
        }
        start = end;
    }
    lines
}
