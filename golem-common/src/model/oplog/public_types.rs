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
use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::DurableFunctionType;
use golem_schema::Empty;

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

/// Parses the string form of `Display` with the cursor's `FromStr`, which the CLI also uses.
impl poem_openapi::types::ParseFromParameter for OplogCursor {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        value.parse::<OplogCursor>().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::OplogCursor;
    use poem_openapi::types::ParseFromParameter;
    use test_r::test;

    #[test]
    fn an_oplog_cursor_round_trips_through_its_string_form_and_its_protobuf_form() {
        [None, Some(""), Some("G1002 AND NOT pending")]
            .into_iter()
            .flat_map(|query| [None, Some(5_000_000)].map(|scan_to| (query, scan_to)))
            .for_each(|(query, scan_to)| {
                let cursor = OplogCursor {
                    next_oplog_index: 120,
                    current_component_revision: 3,
                    query_hash: OplogCursor::query_hash(query),
                    scan_to,
                };
                let parsed = OplogCursor::parse_from_parameter(&cursor.to_string()).unwrap();
                assert_eq!(parsed, cursor);
                let protobuf =
                    golem_api_grpc::proto::golem::worker::OplogCursor::from(cursor.clone());
                assert_eq!(protobuf.scan_to, scan_to);
                assert_eq!(OplogCursor::from(protobuf), cursor);
            });
        // A cursor without `scan_to` keeps its three parts.
        assert_eq!(
            OplogCursor {
                next_oplog_index: 120,
                current_component_revision: 3,
                query_hash: String::new(),
                scan_to: None,
            }
            .to_string(),
            "120-3-"
        );
        // The hash stays the same across processes and releases: it is SHA-256, cut to 8 bytes.
        assert_eq!(OplogCursor::query_hash(Some("")), "e3b0c44298fc1c14");
        assert_eq!(OplogCursor::query_hash(None), "");
    }

    #[test]
    fn an_oplog_cursor_without_a_valid_query_hash_or_search_start_is_invalid() {
        [
            "120-3",
            "120-3-e3b0",
            "120-3-E3B0C44298FC1C14",
            "120-3-e3b0c44298fc1c14-",
            "120-3-e3b0c44298fc1c14-x",
            "120-3-e3b0c44298fc1c14-1-2",
        ]
        .into_iter()
        .for_each(|invalid| {
            assert!(
                OplogCursor::parse_from_parameter(invalid).is_err(),
                "{invalid} must be refused"
            )
        });
    }
}
