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

pub use crate::base_model::oplog::OplogCursor;
use crate::model::Empty;
use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::DurableFunctionType;

pub use crate::base_model::oplog::public_types::*;

impl From<DurableFunctionType> for PublicDurableFunctionType {
    fn from(function_type: DurableFunctionType) -> Self {
        match function_type {
            DurableFunctionType::ReadLocal => PublicDurableFunctionType::ReadLocal(Empty {}),
            DurableFunctionType::WriteLocal => PublicDurableFunctionType::WriteLocal(Empty {}),
            DurableFunctionType::ReadRemote => PublicDurableFunctionType::ReadRemote(Empty {}),
            DurableFunctionType::WriteRemote => PublicDurableFunctionType::WriteRemote(Empty {}),
        }
    }
}

impl From<AttributeValue> for PublicAttributeValue {
    fn from(value: AttributeValue) -> Self {
        match value {
            AttributeValue::String(value) => {
                PublicAttributeValue::String(StringAttributeValue { value })
            }
        }
    }
}

/// Parses the string form `<index>-<revision>-<query hash>` of `Display`.
impl poem_openapi::types::ParseFromParameter for OplogCursor {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        let [index, revision, query_hash] = value.split('-').collect::<Vec<_>>()[..] else {
            return Err("Invalid oplog cursor".into());
        };
        let next_oplog_index = index
            .parse()
            .map_err(|_| "Invalid index in the oplog cursor")?;
        let current_component_revision = revision
            .parse()
            .map_err(|_| "Invalid component revision in the oplog cursor")?;
        let is_hash = query_hash.len() == 16
            && query_hash
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !query_hash.is_empty() && !is_hash {
            return Err("Invalid query hash in the oplog cursor".into());
        }
        Ok(OplogCursor {
            next_oplog_index,
            current_component_revision,
            query_hash: query_hash.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::OplogCursor;
    use poem_openapi::types::ParseFromParameter;
    use test_r::test;

    #[test]
    fn an_oplog_cursor_round_trips_through_its_string_form() {
        for query in [None, Some(""), Some("G1002 AND NOT pending")] {
            let cursor = OplogCursor {
                next_oplog_index: 120,
                current_component_revision: 3,
                query_hash: OplogCursor::query_hash(query),
            };
            let parsed = OplogCursor::parse_from_parameter(&cursor.to_string()).unwrap();
            assert_eq!(parsed, cursor);
        }
        // The hash stays the same across processes and releases: it is SHA-256, cut to 8 bytes.
        assert_eq!(OplogCursor::query_hash(Some("")), "e3b0c44298fc1c14");
        assert_eq!(OplogCursor::query_hash(None), "");
    }

    #[test]
    fn an_oplog_cursor_without_a_valid_query_hash_is_invalid() {
        for invalid in [
            "120-3",
            "120-3-e3b0",
            "120-3-E3B0C44298FC1C14",
            "120-3-e3b0c44298fc1c14-1",
        ] {
            assert!(
                OplogCursor::parse_from_parameter(invalid).is_err(),
                "{invalid} must be refused"
            );
        }
    }
}
