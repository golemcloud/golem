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

use std::time::Duration;

use super::{RetryPolicy, RetryPolicyState, RetryProperties, RetryVerdict, RngSource};

// The state shape comes only from the trusted policy, never from guest bytes.
// The wire payload is a flat list of integers in state traversal order.
fn continuation_fields(
    state: &mut RetryPolicyState,
    field: &mut impl FnMut(u64) -> Result<u64, String>,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), String> {
    if depth >= 64 || *remaining == 0 {
        return Err("Retry continuation exceeds the size/depth limit".into());
    }
    *remaining -= 1;
    match state {
        RetryPolicyState::Counter(n) => {
            *n = field(u64::from(*n))?
                .try_into()
                .map_err(|_| "Invalid retry counter")?;
        }
        RetryPolicyState::Terminal => {}
        RetryPolicyState::Wrapper(inner) => {
            continuation_fields(inner, field, depth + 1, remaining)?;
        }
        RetryPolicyState::CountBox { attempts, inner } => {
            *attempts = field(u64::from(*attempts))?
                .try_into()
                .map_err(|_| "Invalid retry counter")?;
            continuation_fields(inner, field, depth + 1, remaining)?;
        }
        RetryPolicyState::TimeBox { .. } => {
            return Err("Invalid retry continuation state shape".into());
        }
        RetryPolicyState::AndThen {
            left,
            right,
            on_right,
        } => {
            *on_right = match field(u64::from(*on_right))? {
                0 => false,
                1 => true,
                _ => return Err("Invalid retry continuation boolean".into()),
            };
            continuation_fields(left, field, depth + 1, remaining)?;
            continuation_fields(right, field, depth + 1, remaining)?;
        }
        RetryPolicyState::Pair(left, right) => {
            continuation_fields(left, field, depth + 1, remaining)?;
            continuation_fields(right, field, depth + 1, remaining)?;
        }
    }
    Ok(())
}

impl RetryPolicy {
    pub fn next_retry_delay(
        &self,
        token: &[u8],
        elapsed: Duration,
        properties: &RetryProperties,
        rng: &mut dyn RngSource,
    ) -> Result<(Vec<u8>, Option<Duration>), String> {
        let mut policy_bytes = Vec::new();
        desert_rust::serialize(self, &mut policy_bytes).map_err(|e| e.to_string())?;
        let mut previous_state = self.initial_state();
        let mut field_count = 2;
        continuation_fields(
            &mut previous_state,
            &mut |n| {
                field_count += 1;
                Ok(n)
            },
            0,
            &mut 4096,
        )?;
        let mut previous_elapsed = Duration::ZERO;
        if !token.is_empty() {
            let payload = token
                .strip_prefix(policy_bytes.as_slice())
                .ok_or("Retry policy changed during continuation")?;
            // At most one integer per node, plus seconds and nanoseconds. JSON's
            // flat sequence decoder cannot construct recursive state or allocate
            // from an attacker-supplied length prefix.
            if payload.len() > field_count * 21 + 2 {
                return Err("Retry continuation exceeds the size limit".into());
            }
            let fields: Vec<u64> = serde_json::from_slice(payload)
                .map_err(|e| format!("Invalid retry continuation: {e}"))?;
            let mut fields = fields.into_iter();
            let mut next = |_| {
                fields
                    .next()
                    .ok_or_else(|| "Truncated retry continuation".to_string())
            };
            let secs = next(0)?;
            let nanos = next(0)?;
            if nanos >= 1_000_000_000 {
                return Err("Invalid retry continuation nanoseconds".into());
            }
            previous_elapsed = Duration::new(secs, nanos as u32);
            continuation_fields(&mut previous_state, &mut next, 0, &mut 4096)?;
            if fields.next().is_some() {
                return Err("Trailing retry continuation fields".into());
            }
        }
        let elapsed = elapsed.max(previous_elapsed);
        let (mut state, verdict) = self.step(&previous_state, elapsed, properties, rng);
        let delay = match verdict {
            RetryVerdict::Retry(delay) => Some(delay),
            RetryVerdict::GiveUp => None,
            RetryVerdict::Error(error) => return Err(error.to_string()),
        };
        let mut fields = vec![elapsed.as_secs(), u64::from(elapsed.subsec_nanos())];
        continuation_fields(
            &mut state,
            &mut |n| {
                fields.push(n);
                Ok(n)
            },
            0,
            &mut 4096,
        )?;
        serde_json::to_writer(&mut policy_bytes, &fields).map_err(|e| e.to_string())?;
        Ok((policy_bytes, delay))
    }
}
