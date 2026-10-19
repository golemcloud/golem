// Copyright 2024-2026 Golem Cloud
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

/// A completed command's shell-visible status, distinct from a failure to execute the tool.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[schema(named = "golem.tool.ProcessExitStatus")]
#[cfg_attr(
    feature = "guest",
    derive(
        golem_schema_derive::WireSchema,
        golem_schema_derive::IntoWire,
        golem_schema_derive::FromWire
    )
)]
pub struct ProcessExitStatus {
    pub exit_code: u8,
}

#[cfg(test)]
mod tests {
    use super::ProcessExitStatus;
    use crate::{FromSchema, IntoSchema, SchemaType, SchemaValue, try_into_schema_graph};
    use test_r::test;

    #[test]
    fn process_exit_status_has_the_cross_language_named_contract() {
        let graph = try_into_schema_graph::<ProcessExitStatus>().unwrap();
        assert_eq!(
            graph.root,
            SchemaType::ref_to("golem.tool.ProcessExitStatus".into())
        );
        let SchemaType::Record { fields, .. } = graph.resolve_ref(&graph.root).unwrap() else {
            panic!("process status must be a record");
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "exit_code");
        assert_eq!(fields[0].body, SchemaType::u8());
        for code in [0, 7, 255] {
            let status = ProcessExitStatus { exit_code: code };
            let value = SchemaValue::Record {
                fields: vec![SchemaValue::U8(code)],
            };
            assert_eq!(status.to_value(), value);
            assert_eq!(ProcessExitStatus::from_value(&value).unwrap(), status);
        }
    }
}
