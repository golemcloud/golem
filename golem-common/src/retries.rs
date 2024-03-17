// Copyright 2024 Golem Cloud
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

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tracing::{error, warn};

use crate::config::RetryConfig;
use crate::metrics::external_calls::{
    record_external_call_failure, record_external_call_retry, record_external_call_success,
};

/// Returns the delay to be waited before the next retry attempt.
/// To be called after a failed attempt, with the number of attempts so far.
/// Returns None if the maximum number of attempts has been reached.
pub fn get_delay(config: &RetryConfig, attempts: u64) -> Option<Duration> {
    // Exponential backoff algorithm inspired by fred::pool::ReconnectPolicy::Exponential
    if attempts >= (config.max_attempts as u64) {
        return None;
    }

    let base_delay = (config.multiplier as u64)
        .saturating_pow(attempts.checked_sub(1).unwrap_or(0).try_into().unwrap_or(0))
        .saturating_mul(config.min_delay.as_millis() as u64);

    let delay = Duration::from_millis(std::cmp::min(
        config.max_delay.as_millis() as u64,
        base_delay,
    ));
    Some(delay)
}

pub async fn with_retries<'a, In, F, G, R, E>(
    description: &str,
    target_label: &'static str,
    op_label: &'static str,
    config: &RetryConfig,
    i: &In,
    action: F,
    is_retriable: G,
) -> Result<R, E>
where
    E: std::error::Error,
    F: for<'b> Fn(&'b In) -> Pin<Box<dyn Future<Output = Result<R, E>> + 'b + Send>>,
    G: Fn(&E) -> bool,
{
    let mut attempts = 0;
    loop {
        attempts += 1;
        let start = Instant::now();
        let r = action(i).await;
        let end = Instant::now();
        let duration = end.duration_since(start);
        match r {
            Ok(result) => {
                record_external_call_success(target_label, op_label, duration);
                return Ok(result);
            }
            Err(error) if is_retriable(&error) => {
                if let Some(delay) = get_delay(config, attempts) {
                    warn!(
                        "{} failed after {} attempts with {}, retrying in {:?}",
                        description, attempts, error, delay
                    );
                    record_external_call_retry(target_label, op_label);
                    tokio::time::sleep(delay).await;
                } else {
                    error!(
                        "{} failed after {} attempts with {}",
                        description, attempts, error
                    );
                    record_external_call_failure(target_label, op_label);
                    return Err(error);
                }
            }
            Err(error) => {
                error!(
                    "{} failed with non-retriable error after {} attempts with {}",
                    description, attempts, error
                );
                record_external_call_failure(target_label, op_label);
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::config::RetryConfig;

    #[test]
    pub fn get_delay_example1() {
        let config = RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2,
        };

        let mut delays: Vec<Duration> = Vec::new();
        let mut attempts = 0;

        capture_delays(&config, &mut attempts, &mut delays);

        assert_eq!(attempts, 5);
        assert_eq!(
            delays,
            vec![
                Duration::from_millis(100), // after 1st attempt
                Duration::from_millis(200), // after 2nd attempt
                Duration::from_millis(400), // after 3rd attempt
                Duration::from_millis(800), // after 4th attempt
            ]
        )
    }

    fn capture_delays(config: &RetryConfig, attempts: &mut u64, delays: &mut Vec<Duration>) {
        loop {
            *attempts += 1;
            let delay = super::get_delay(config, *attempts);
            if let Some(delay) = delay {
                delays.push(delay);
            } else {
                break;
            }
        }
    }
}
