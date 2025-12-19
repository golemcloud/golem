// Copyright 2024-2025 Golem Cloud
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

use crate::golem_wasm::{WitNode, WitValue};
use golem_wasm::NodeIndex;

// Allows unwrapping a wit-value which is a tuple with a single element
// into just that element's wit-value.
// This is useful for simplifying the handling of return values from rpc calls,
// as rpc result is often represented using a tuple with one element.
pub fn unwrap_wit_tuple(wit_value: WitValue) -> WitValue {
    if let Some(WitNode::TupleValue(nodes)) = wit_value.nodes.first() {
        if nodes.len() == 1 {
            if let Some(&first_index) = nodes.first() {
                let first_index = first_index as usize;
                let mut new_nodes = wit_value.nodes[first_index..].to_vec();
                rebase_indices(&mut new_nodes, first_index);
                return WitValue { nodes: new_nodes };
            }
        }
    }
    wit_value
}

fn rebase_indices(nodes: &mut [WitNode], base_index: usize) {
    for node in nodes.iter_mut() {
        match node {
            WitNode::TupleValue(indices)
            | WitNode::ListValue(indices)
            | WitNode::RecordValue(indices) => {
                for idx in indices.iter_mut() {
                    *idx -= base_index as NodeIndex;
                }
            }
            WitNode::VariantValue((_, Some(idx))) | WitNode::OptionValue(Some(idx)) => {
                *idx -= base_index as NodeIndex;
            }
            WitNode::ResultValue(Ok(Some(idx)) | Err(Some(idx))) => {
                *idx -= base_index as NodeIndex;
            }
            WitNode::VariantValue((_, None))
            | WitNode::OptionValue(None)
            | WitNode::ResultValue(Ok(None) | Err(None))
            | WitNode::FlagsValue(_)
            | WitNode::EnumValue(_)
            | WitNode::PrimU8(_)
            | WitNode::PrimU16(_)
            | WitNode::PrimU32(_)
            | WitNode::PrimU64(_)
            | WitNode::PrimS8(_)
            | WitNode::PrimS16(_)
            | WitNode::PrimS32(_)
            | WitNode::PrimS64(_)
            | WitNode::PrimFloat32(_)
            | WitNode::PrimFloat64(_)
            | WitNode::PrimChar(_)
            | WitNode::PrimBool(_)
            | WitNode::PrimString(_)
            | WitNode::Handle(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agentic::unwrap_wit_tuple;
    use golem_wasm::{Value, WitValue};
    use test_r::test;

    #[test]
    fn test_unwrap_wit_tuple() {
        use Value::*;

        let cases: Vec<Value> = vec![
            Bool(true),
            U8(42),
            U16(4242),
            U32(424242),
            U64(42424242),
            S8(-42),
            S16(-4242),
            S32(-424242),
            S64(-42424242),
            F32(3.15),
            F64(-2.719),
            Char('x'),
            String("hello".to_string()),
            List(vec![U8(1), U8(2)]),
            Tuple(vec![]),
            Tuple(vec![Bool(true)]),
            Tuple(vec![Bool(true), Bool(false)]),
            Record(vec![U8(1), U8(2)]),
            Variant {
                case_idx: 0,
                case_value: Some(Box::new(U8(10))),
            },
            Enum(2),
            Flags(vec![true, false, true]),
            Option(Some(Box::new(String("opt".to_string())))),
            Option(None),
            Result(Ok(Some(Box::new(U8(5))))),
            Result(Err(Some(Box::new(U8(6))))),
            Result(Ok(None)),
            Result(Err(None)),
            Handle {
                uri: "uri".to_string(),
                resource_id: 99,
            },
        ];

        for value in cases {
            let wit_value = WitValue::from(value.clone());
            let unwrapped = unwrap_wit_tuple(wit_value);
            let round_trip_value = Value::from(unwrapped);

            if let Tuple(inner) = &value {
                if inner.len() == 1 {
                    assert_eq!(round_trip_value, inner[0].clone());
                    continue;
                }
            }

            assert_eq!(round_trip_value, value);
        }
    }
}
