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

use std::collections::BTreeMap;

pub fn validate(_source: &str) -> anyhow::Result<()> {
    Ok(())
}

pub fn merge_guides(current: &str, new: &str) -> anyhow::Result<String> {
    let mut new_guides = extract_managed_guides(new);
    if new_guides.is_empty() {
        return Ok(current.to_string());
    }

    let current_sections = extract_guide_sections(current);
    if current_sections.is_empty() {
        let mut merged = current.trim_end().to_string();
        if !merged.is_empty() {
            merged.push_str("\n\n");
        }
        merged.push_str(&render_guides(&new_guides));
        merged.push('\n');
        return Ok(merged);
    }

    let mut merged = String::new();
    let mut cursor = 0;
    for section in &current_sections {
        merged.push_str(&current[cursor..section.start]);
        let content = new_guides
            .remove(&section.key)
            .unwrap_or_else(|| section.content.clone());
        merged.push_str(&render_guide(&section.key, &content));
        cursor = section.end;
    }
    merged.push_str(&current[cursor..]);

    if !new_guides.is_empty() {
        if !merged.ends_with('\n') {
            merged.push('\n');
        }
        if !merged.ends_with("\n\n") {
            merged.push('\n');
        }
        merged.push_str(&render_guides(&new_guides));
    }

    if !merged.ends_with('\n') {
        merged.push('\n');
    }

    Ok(merged)
}

fn guide_start_marker(key: &str) -> String {
    format!("<!-- golem-managed:guide:{key}:start -->")
}

fn guide_end_marker(key: &str) -> String {
    format!("<!-- golem-managed:guide:{key}:end -->")
}

fn extract_managed_guides(source: &str) -> BTreeMap<String, String> {
    extract_guide_sections(source)
        .into_iter()
        .map(|section| (section.key, section.content))
        .collect()
}

#[derive(Debug, Clone)]
struct GuideSection {
    key: String,
    start: usize,
    end: usize,
    content: String,
}

fn extract_guide_sections(source: &str) -> Vec<GuideSection> {
    let mut result = BTreeMap::new();

    let mut cursor = 0;
    while let Some(start_idx_rel) = source[cursor..].find("<!-- golem-managed:guide:") {
        let start_idx = cursor + start_idx_rel;
        let Some(marker_end_rel) = source[start_idx..].find(" -->") else {
            break;
        };
        let marker_end = start_idx + marker_end_rel + 4;
        let marker = &source[start_idx..marker_end];

        let Some(key) = extract_guide_key(marker, ":start") else {
            cursor = marker_end;
            continue;
        };

        let content_start = marker_end;
        let end_marker = guide_end_marker(&key);
        let Some(end_marker_idx_rel) = source[content_start..].find(&end_marker) else {
            cursor = marker_end;
            continue;
        };
        let end_marker_idx = content_start + end_marker_idx_rel;
        let section_end = end_marker_idx + end_marker.len();
        let section_end = if source[section_end..].starts_with('\n') {
            section_end + 1
        } else {
            section_end
        };

        let content = source[content_start..end_marker_idx]
            .trim_start_matches('\n')
            .trim_end_matches('\n')
            .to_string();

        if !result.contains_key(&key) {
            result.insert(key.clone(), (start_idx, section_end, content.clone()));
        }
        cursor = section_end;
    }

    result
        .into_iter()
        .map(|(key, (start, end, content))| GuideSection {
            key,
            start,
            end,
            content,
        })
        .collect()
}

fn extract_guide_key(marker: &str, suffix: &str) -> Option<String> {
    let prefix = "<!-- golem-managed:guide:";
    let suffix = format!("{suffix} -->");
    marker
        .strip_prefix(prefix)
        .and_then(|s| s.strip_suffix(&suffix))
        .map(ToString::to_string)
}

fn render_guide(key: &str, content: &str) -> String {
    format!(
        "{}\n{}\n{}\n",
        guide_start_marker(key),
        content.trim_end(),
        guide_end_marker(key)
    )
}

fn render_guides(guides: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    for (idx, (key, content)) in guides.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&render_guide(key, content));
    }
    out
}
