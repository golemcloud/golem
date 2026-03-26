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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem_api_1_x::retry::{
    AddDelayConfig, ClampConfig, CountBoxConfig, ExponentialConfig, FibonacciConfig,
    FilteredConfig, Host, JitterConfig, NamedRetryPolicy as WitNamedRetryPolicy, PolicyNode,
    PredicateNode, PredicateValue as WitPredicateValue, PropertyComparison, PropertyPattern,
    PropertySetCheck, RetryPolicy as WitRetryPolicy, RetryPredicate, TimeBoxConfig,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::retry_policy::{
    duration_to_nanos, NamedRetryPolicy, Predicate, PredicateValue, RetryContext, RetryPolicy,
};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_retry_policies(&mut self) -> anyhow::Result<Vec<WitNamedRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "get_retry_policies");
        let policies = self.state.named_retry_policies();
        Ok(policies.iter().cloned().map(|p| p.into()).collect())
    }

    async fn get_retry_policy_by_name(
        &mut self,
        name: String,
    ) -> anyhow::Result<Option<WitNamedRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "get_retry_policy_by_name");
        let policies = self.state.named_retry_policies();
        Ok(policies.iter().find(|p| p.name == name).cloned().map(|p| p.into()))
    }

    async fn resolve_retry_policy(
        &mut self,
        verb: String,
        noun_uri: String,
        properties: Vec<(String, WitPredicateValue)>,
    ) -> anyhow::Result<Option<WitRetryPolicy>> {
        self.observe_function_call("golem::api::retry", "resolve_retry_policy");

        let mut props = RetryContext::custom(&verb, &noun_uri);
        for (key, value) in properties {
            props.set(key, wit_predicate_value_to_model(value));
        }

        let policies = self.state.named_retry_policies();
        match NamedRetryPolicy::resolve(policies, &props) {
            Ok(Some(matched)) => Ok(Some(matched.policy.clone().into())),
            Ok(None) => Ok(None),
            Err(err) => Err(anyhow::anyhow!("Retry policy resolution error: {err}")),
        }
    }

    async fn set_retry_policy(
        &mut self,
        _policy: WitNamedRetryPolicy,
    ) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "set_retry_policy");
        // PR 2 will introduce oplog entry types for durable named policy mutation.
        // For now, this is a no-op stub — components cannot yet persist runtime
        // policy changes via this interface.
        Err(anyhow::anyhow!(
            "set-retry-policy is not yet implemented; named policies are currently read-only and configured via agent config"
        ))
    }

    async fn remove_retry_policy(&mut self, _name: String) -> anyhow::Result<()> {
        self.observe_function_call("golem::api::retry", "remove_retry_policy");
        // PR 2 will introduce oplog entry types for durable named policy mutation.
        Err(anyhow::anyhow!(
            "remove-retry-policy is not yet implemented; named policies are currently read-only and configured via agent config"
        ))
    }
}

fn wit_predicate_value_to_model(v: WitPredicateValue) -> PredicateValue {
    match v {
        WitPredicateValue::Text(s) => PredicateValue::Text(s),
        WitPredicateValue::Integer(i) => PredicateValue::Integer(i),
        WitPredicateValue::Boolean(b) => PredicateValue::Boolean(b),
    }
}

fn model_predicate_value_to_wit(v: &PredicateValue) -> WitPredicateValue {
    match v {
        PredicateValue::Text(s) => WitPredicateValue::Text(s.clone()),
        PredicateValue::Integer(i) => WitPredicateValue::Integer(*i),
        PredicateValue::Boolean(b) => WitPredicateValue::Boolean(*b),
    }
}

// ── Model → WIT conversions ─────────────────────────────────────────

impl From<NamedRetryPolicy> for WitNamedRetryPolicy {
    fn from(p: NamedRetryPolicy) -> Self {
        Self {
            name: p.name,
            priority: p.priority,
            predicate: predicate_to_wit(p.predicate),
            policy: policy_to_wit(p.policy),
        }
    }
}

impl From<RetryPolicy> for WitRetryPolicy {
    fn from(p: RetryPolicy) -> Self {
        policy_to_wit(p)
    }
}

fn predicate_to_wit(pred: Predicate) -> RetryPredicate {
    let mut nodes = Vec::new();
    push_predicate_node_wit(pred, &mut nodes);
    RetryPredicate { nodes }
}

fn push_predicate_node_wit(pred: Predicate, nodes: &mut Vec<PredicateNode>) -> i32 {
    let idx = nodes.len() as i32;
    // placeholder
    nodes.push(PredicateNode::PredTrue);

    let node = match pred {
        Predicate::PropEq { property, value } => PredicateNode::PropEq(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropNeq { property, value } => PredicateNode::PropNeq(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropGt { property, value } => PredicateNode::PropGt(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropGte { property, value } => PredicateNode::PropGte(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropLt { property, value } => PredicateNode::PropLt(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropLte { property, value } => PredicateNode::PropLte(PropertyComparison {
            property_name: property,
            value: model_predicate_value_to_wit(&value),
        }),
        Predicate::PropExists(property) => PredicateNode::PropExists(property),
        Predicate::PropIn { property, values } => PredicateNode::PropIn(PropertySetCheck {
            property_name: property,
            values: values.iter().map(model_predicate_value_to_wit).collect(),
        }),
        Predicate::PropMatches { property, pattern } => {
            PredicateNode::PropMatches(PropertyPattern {
                property_name: property,
                pattern,
            })
        }
        Predicate::PropStartsWith { property, prefix } => {
            PredicateNode::PropStartsWith(PropertyPattern {
                property_name: property,
                pattern: prefix,
            })
        }
        Predicate::PropContains {
            property,
            substring,
        } => PredicateNode::PropContains(PropertyPattern {
            property_name: property,
            pattern: substring,
        }),
        Predicate::And(l, r) => {
            let li = push_predicate_node_wit(*l, nodes);
            let ri = push_predicate_node_wit(*r, nodes);
            PredicateNode::PredAnd((li, ri))
        }
        Predicate::Or(l, r) => {
            let li = push_predicate_node_wit(*l, nodes);
            let ri = push_predicate_node_wit(*r, nodes);
            PredicateNode::PredOr((li, ri))
        }
        Predicate::Not(inner) => {
            let i = push_predicate_node_wit(*inner, nodes);
            PredicateNode::PredNot(i)
        }
        Predicate::True => PredicateNode::PredTrue,
        Predicate::False => PredicateNode::PredFalse,
    };

    nodes[idx as usize] = node;
    idx
}

fn policy_to_wit(policy: RetryPolicy) -> WitRetryPolicy {
    let mut nodes = Vec::new();
    push_policy_node_wit(policy, &mut nodes);
    WitRetryPolicy { nodes }
}

fn push_policy_node_wit(policy: RetryPolicy, nodes: &mut Vec<PolicyNode>) -> i32 {
    let idx = nodes.len() as i32;
    // placeholder
    nodes.push(PolicyNode::Never);

    let node = match policy {
        RetryPolicy::Periodic(d) => PolicyNode::Periodic(duration_to_nanos(d)),
        RetryPolicy::Exponential { base_delay, factor } => {
            PolicyNode::Exponential(ExponentialConfig {
                base_delay: duration_to_nanos(base_delay),
                factor,
            })
        }
        RetryPolicy::Fibonacci { first, second } => PolicyNode::Fibonacci(FibonacciConfig {
            first: duration_to_nanos(first),
            second: duration_to_nanos(second),
        }),
        RetryPolicy::Immediate => PolicyNode::Immediate,
        RetryPolicy::Never => PolicyNode::Never,
        RetryPolicy::CountBox { max_retries, inner } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::CountBox(CountBoxConfig {
                max_retries,
                inner: i,
            })
        }
        RetryPolicy::TimeBox { limit, inner } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::TimeBox(TimeBoxConfig {
                limit: duration_to_nanos(limit),
                inner: i,
            })
        }
        RetryPolicy::Clamp {
            min_delay,
            max_delay,
            inner,
        } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::ClampDelay(ClampConfig {
                min_delay: duration_to_nanos(min_delay),
                max_delay: duration_to_nanos(max_delay),
                inner: i,
            })
        }
        RetryPolicy::AddDelay { delay, inner } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::AddDelay(AddDelayConfig {
                delay: duration_to_nanos(delay),
                inner: i,
            })
        }
        RetryPolicy::Jitter { factor, inner } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::Jitter(JitterConfig { factor, inner: i })
        }
        RetryPolicy::FilteredOn { predicate, inner } => {
            let i = push_policy_node_wit(*inner, nodes);
            PolicyNode::FilteredOn(FilteredConfig {
                predicate: predicate_to_wit(predicate),
                inner: i,
            })
        }
        RetryPolicy::AndThen(l, r) => {
            let li = push_policy_node_wit(*l, nodes);
            let ri = push_policy_node_wit(*r, nodes);
            PolicyNode::AndThen((li, ri))
        }
        RetryPolicy::Union(l, r) => {
            let li = push_policy_node_wit(*l, nodes);
            let ri = push_policy_node_wit(*r, nodes);
            PolicyNode::PolicyUnion((li, ri))
        }
        RetryPolicy::Intersect(l, r) => {
            let li = push_policy_node_wit(*l, nodes);
            let ri = push_policy_node_wit(*r, nodes);
            PolicyNode::PolicyIntersect((li, ri))
        }
    };

    nodes[idx as usize] = node;
    idx
}
