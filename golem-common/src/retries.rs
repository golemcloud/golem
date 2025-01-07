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

use rand::{thread_rng, Rng};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};
use tracing::{error, info, warn, Level};

use crate::metrics::external_calls::{
    record_external_call_failure, record_external_call_retry, record_external_call_success,
};
use crate::model::RetryConfig;
use crate::retriable_error::IsRetriableError;

/// Returns the delay to be waited before the next retry attempt.
/// To be called after a failed attempt, with the number of attempts so far.
/// Returns None if the maximum number of attempts has been reached.
pub fn get_delay(config: &RetryConfig, attempts: u64) -> Option<Duration> {
    // Exponential backoff algorithm inspired by fred::pool::ReconnectPolicy::Exponential
    // Unlike fred, max jitter is not a static value, rather proportional to the current calculated delay
    if attempts >= (config.max_attempts as u64) {
        return None;
    }

    let delay_with_opt_jitter = {
        let base_delay = (config.multiplier as u64)
            .saturating_pow(attempts.saturating_sub(1).try_into().unwrap_or(0))
            .saturating_mul(config.min_delay.as_millis() as u64);

        match config.max_jitter_factor {
            Some(max_jitter_factor) => {
                let jitter_factor = thread_rng().gen_range(0.0f64..max_jitter_factor);
                base_delay.saturating_add((base_delay as f64 * jitter_factor) as u64)
            }
            None => base_delay,
        }
    };

    let delay = Duration::from_millis(std::cmp::min(
        config.max_delay.as_millis() as u64,
        delay_with_opt_jitter,
    ));
    Some(delay)
}

/// Lower level support for performing the same retry logic configured by `RetryConfig`
/// as `with_retries`, but without being a higher order function and without doing any
/// logging and metrics.
///
/// Before attempting to perform the retriable action, call `start_attempt`. If it fails,
/// call `failed_attempt` and if that returns true, start a new attempt immediately.
pub struct RetryState<'a> {
    attempts: u64,
    retry_config: &'a RetryConfig,
}

impl<'a> RetryState<'a> {
    /// Initializes the retry state.
    pub fn new(retry_config: &'a RetryConfig) -> Self {
        Self {
            attempts: 0,
            retry_config,
        }
    }

    /// Indicates a new attempt has started
    pub fn start_attempt(&mut self) {
        self.attempts += 1;
    }

    /// Indicates that the started attempt has failed. If there are still retries possible,
    /// this function will sleep for the calculated delay and then return true. If there
    /// are no more retry attempts, it returns false
    pub async fn failed_attempt(&self) -> bool {
        if let Some(delay) = get_delay(self.retry_config, self.attempts) {
            tokio::time::sleep(delay).await;
            true
        } else {
            false
        }
    }
}

pub async fn with_retries<In, F, G, R, E>(
    target_label: &'static str,
    op_label: &'static str,
    op_id: Option<String>,
    config: &RetryConfig,
    i: &In,
    action: F,
    is_retriable: G,
) -> Result<R, E>
where
    E: std::error::Error,
    F: for<'a> Fn(&'a In) -> Pin<Box<dyn Future<Output = Result<R, E>> + 'a + Send>>,
    G: Fn(&E) -> bool,
{
    with_retries_customized(
        target_label,
        op_label,
        op_id,
        config,
        i,
        action,
        is_retriable,
        |error| Some(error.to_string()),
    )
    .await
}

pub async fn with_retries_customized<In, F, R, E>(
    target_label: &'static str,
    op_label: &'static str,
    op_id: Option<String>,
    config: &RetryConfig,
    i: &In,
    action: F,
    is_retriable: impl Fn(&E) -> bool,
    as_loggable: impl Fn(&E) -> Option<String>,
) -> Result<R, E>
where
    F: for<'a> Fn(&'a In) -> Pin<Box<dyn Future<Output = Result<R, E>> + 'a + Send>>,
{
    let mut attempts = 0;
    loop {
        attempts += 1;

        let start = Instant::now();
        let r = action(i).await;
        let end = Instant::now();
        let duration = end.duration_since(start);

        let span = tracing::span!(
            Level::INFO,
            "retry",
            target = target_label,
            op = op_label,
            op_id,
            attempt = attempts
        );
        let enter = span.enter();

        let delay = match r {
            Ok(result) => {
                info!(duration_ms = duration.as_millis(), "op success");
                record_external_call_success(target_label, op_label, duration);
                return Ok(result);
            }
            Err(error) if is_retriable(&error) => {
                if let Some(delay) = get_delay(config, attempts) {
                    if let Some(error_string) = as_loggable(&error) {
                        warn!(
                            delay_ms = delay.as_millis(),
                            error = error_string,
                            "op failure - retrying"
                        );
                        record_external_call_retry(target_label, op_label);
                    }
                    delay
                } else {
                    if let Some(error_string) = as_loggable(&error) {
                        error!(error = error_string, "op failure - no more retries");
                        record_external_call_failure(target_label, op_label);
                    }
                    return Err(error);
                }
            }
            Err(error) => {
                if let Some(error_string) = as_loggable(&error) {
                    error!(error = error_string, "op failure - non-retriable");
                    record_external_call_failure(target_label, op_label);
                }
                return Err(error);
            }
        };

        drop(enter);

        tokio::time::sleep(delay).await;
    }
}

pub async fn with_retriable_errors<In, F, R, E>(
    target_label: &'static str,
    op_label: &'static str,
    op_id: Option<String>,
    config: &RetryConfig,
    i: &In,
    action: F,
) -> Result<R, E>
where
    E: std::error::Error + IsRetriableError,
    F: for<'a> Fn(&'a In) -> Pin<Box<dyn Future<Output = Result<R, E>> + 'a + Send>>,
{
    with_retries_customized(
        target_label,
        op_label,
        op_id,
        config,
        i,
        action,
        IsRetriableError::is_retriable,
        IsRetriableError::as_loggable,
    )
    .await
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::RetryConfig;
    use std::time::Duration;

    #[test]
    pub fn get_delay_example_without_jitter() {
        let config = RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2.0,
            max_jitter_factor: None,
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

    #[test]
    pub fn get_delay_example_with_jitter() {
        let config = RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2.0,
            max_jitter_factor: Some(0.1),
        };

        let mut delays: Vec<Duration> = Vec::new();
        let mut attempts = 0;

        capture_delays(&config, &mut attempts, &mut delays);
        assert_eq!(attempts, 5);

        let expected_base_delays = vec![
            Duration::from_millis(100), // after 1st attempt
            Duration::from_millis(200), // after 2nd attempt
            Duration::from_millis(400), // after 3rd attempt
            Duration::from_millis(800), // after 4th attempt
        ];
        assert_eq!(delays.len(), expected_base_delays.len());

        for (expected_base_delay, actual_delay) in expected_base_delays.into_iter().zip(delays) {
            assert!(
                expected_base_delay <= actual_delay,
                "{expected_base_delay:?} <= {actual_delay:?}"
            );
            let upper_bound_delay = expected_base_delay
                + Duration::from_millis((expected_base_delay.as_millis() as f64 * 0.15) as u64);
            assert!(
                actual_delay <= upper_bound_delay,
                "{actual_delay:?} <= {upper_bound_delay:?}"
            );
        }
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
