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

use super::*;

impl From<golem_common::model::RetryPolicyState> for oplog::RetryPolicyState {
    fn from(state: golem_common::model::RetryPolicyState) -> Self {
        let mut nodes = Vec::new();
        push_wit_state_node(state, &mut nodes);
        Self { nodes }
    }
}

impl From<oplog::RetryPolicyState> for golem_common::model::RetryPolicyState {
    fn from(wit: oplog::RetryPolicyState) -> Self {
        build_wit_state_from_index(&wit.nodes, 0).unwrap_or(Self::Terminal)
    }
}

fn push_wit_state_node(
    state: golem_common::model::RetryPolicyState,
    nodes: &mut Vec<oplog::StateNode>,
) -> i32 {
    use golem_common::model::RetryPolicyState;
    let index = nodes.len() as i32;
    nodes.push(oplog::StateNode::Terminal);

    let node = match state {
        RetryPolicyState::Counter(n) => oplog::StateNode::Counter(n),
        RetryPolicyState::Terminal => oplog::StateNode::Terminal,
        RetryPolicyState::Wrapper(inner) => {
            let inner_idx = push_wit_state_node(*inner, nodes);
            oplog::StateNode::Wrapper(inner_idx)
        }
        RetryPolicyState::CountBox { attempts, inner } => {
            let inner_idx = push_wit_state_node(*inner, nodes);
            oplog::StateNode::CountBox(oplog::CountBoxState {
                attempts,
                inner: inner_idx,
            })
        }
        RetryPolicyState::TimeBox { inner, .. } => {
            let inner_idx = push_wit_state_node(*inner, nodes);
            oplog::StateNode::Wrapper(inner_idx)
        }
        RetryPolicyState::AndThen {
            left,
            right,
            on_right,
        } => {
            let left_idx = push_wit_state_node(*left, nodes);
            let right_idx = push_wit_state_node(*right, nodes);
            oplog::StateNode::AndThen(oplog::AndThenState {
                left: left_idx,
                right: right_idx,
                on_right,
            })
        }
        RetryPolicyState::Pair(left, right) => {
            let left_idx = push_wit_state_node(*left, nodes);
            let right_idx = push_wit_state_node(*right, nodes);
            oplog::StateNode::Pair(oplog::PairState {
                left: left_idx,
                right: right_idx,
            })
        }
    };

    nodes[index as usize] = node;
    index
}

fn build_wit_state_from_index(
    nodes: &[oplog::StateNode],
    index: i32,
) -> Option<golem_common::model::RetryPolicyState> {
    use golem_common::model::RetryPolicyState;
    if index < 0 || (index as usize) >= nodes.len() {
        return None;
    }

    match &nodes[index as usize] {
        oplog::StateNode::Counter(n) => Some(RetryPolicyState::Counter(*n)),
        oplog::StateNode::Terminal => Some(RetryPolicyState::Terminal),
        oplog::StateNode::Wrapper(inner) => Some(RetryPolicyState::Wrapper(Box::new(
            build_wit_state_from_index(nodes, *inner)?,
        ))),
        oplog::StateNode::CountBox(oplog::CountBoxState { attempts, inner }) => {
            Some(RetryPolicyState::CountBox {
                attempts: *attempts,
                inner: Box::new(build_wit_state_from_index(nodes, *inner)?),
            })
        }
        oplog::StateNode::AndThen(oplog::AndThenState {
            left,
            right,
            on_right,
        }) => Some(RetryPolicyState::AndThen {
            left: Box::new(build_wit_state_from_index(nodes, *left)?),
            right: Box::new(build_wit_state_from_index(nodes, *right)?),
            on_right: *on_right,
        }),
        oplog::StateNode::Pair(oplog::PairState { left, right }) => Some(RetryPolicyState::Pair(
            Box::new(build_wit_state_from_index(nodes, *left)?),
            Box::new(build_wit_state_from_index(nodes, *right)?),
        )),
    }
}
