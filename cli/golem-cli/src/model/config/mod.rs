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

pub fn value_at_path<'a>(
    root: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            _ => return None,
        };
    }
    Some(current)
}

pub fn collect_leaf_paths(value: &serde_json::Value) -> Vec<Vec<String>> {
    fn collect(value: &serde_json::Value, prefix: &mut Vec<String>, result: &mut Vec<Vec<String>>) {
        match value {
            serde_json::Value::Object(map) => {
                if map.is_empty() {
                    result.push(prefix.clone());
                }
                for (key, nested) in map {
                    prefix.push(key.clone());
                    collect(nested, prefix, result);
                    prefix.pop();
                }
            }
            _ => result.push(prefix.clone()),
        }
    }

    let mut result = Vec::new();
    collect(value, &mut vec![], &mut result);
    result
}

#[cfg(test)]
mod test {
    use super::{collect_leaf_paths, value_at_path};
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn value_at_path_returns_nested_values() {
        let input = json!({
            "a": {
                "b": {
                    "c": 42
                }
            }
        });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string(), "c".to_string()]),
            Some(&json!(42))
        );
    }

    #[test]
    fn value_at_path_returns_none_for_non_objects() {
        let input = json!({ "a": 1 });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string()]),
            None
        );
    }

    #[test]
    fn collect_leaf_paths_collects_all_terminal_paths() {
        let input = json!({
            "a": { "x": 1, "y": true },
            "b": [1,2],
            "c": "v"
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["a.x", "a.y", "b", "c"]);
    }

    #[test]
    fn collect_leaf_paths_treats_empty_object_as_terminal() {
        let input = json!({
            "db": {},
            "nested": {
                "conn": {}
            }
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["db", "nested.conn"]);
    }
}
