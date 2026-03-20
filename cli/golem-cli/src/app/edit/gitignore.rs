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

use itertools::Itertools;
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn merge(source: &str, additional: &str) -> String {
    let lines = source
        .lines()
        .chain(additional.lines())
        .map(str::to_string)
        .collect::<Vec<_>>();

    let mut polarity_by_pattern = HashMap::<String, (bool, bool)>::new();
    for line in &lines {
        if let Some((negated, pattern)) = parse_pattern_line(line) {
            let entry = polarity_by_pattern.entry(pattern).or_insert((false, false));
            if negated {
                entry.0 = true;
            } else {
                entry.1 = true;
            }
        }
    }

    let mut ordered = Vec::new();
    let mut ordered_seen = HashSet::new();
    let mut sortable = BTreeSet::new();

    for line in lines {
        let is_conflicting = match parse_pattern_line(&line) {
            Some((_, pattern)) => {
                let (has_negated, has_non_negated) = polarity_by_pattern
                    .get(&pattern)
                    .copied()
                    .unwrap_or((false, false));
                has_negated && has_non_negated
            }
            None => true,
        };

        if is_conflicting {
            if ordered_seen.insert(line.clone()) {
                ordered.push(line);
            }
        } else {
            sortable.insert(line);
        }
    }

    ordered.into_iter().chain(sortable).join("\n")
}

fn parse_pattern_line(line: &str) -> Option<(bool, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    if let Some(pattern) = trimmed.strip_prefix('!') {
        if pattern.is_empty() {
            return None;
        }
        return Some((true, pattern.to_string()));
    }

    Some((false, trimmed.to_string()))
}
