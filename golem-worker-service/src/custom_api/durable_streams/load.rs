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

use crate::config::DurableStreamsLoadConfig;
use http::StatusCode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RATE_WINDOW: Duration = Duration::from_secs(1);
const MAX_TRACKED_RATE_STREAMS: usize = 16_384;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadRejection {
    PerStreamReaders,
    PerNodeReaders,
    CatchUpRate,
    AppendRate,
}

impl LoadRejection {
    pub fn status_code(self) -> StatusCode {
        match self {
            Self::PerStreamReaders | Self::PerNodeReaders => StatusCode::SERVICE_UNAVAILABLE,
            Self::CatchUpRate | Self::AppendRate => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    fn metric_reason(self) -> &'static str {
        match self {
            Self::PerStreamReaders => "per-stream-reader-limit",
            Self::PerNodeReaders => "per-node-reader-limit",
            Self::CatchUpRate => "catch-up-rate-limit",
            Self::AppendRate => "append-rate-limit",
        }
    }
}

#[derive(Clone)]
pub struct DurableStreamLoadLimiter {
    inner: Arc<Inner>,
}

struct Inner {
    config: DurableStreamsLoadConfig,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    readers_by_stream: HashMap<String, usize>,
    readers_on_node: usize,
    catch_up_by_stream: HashMap<String, RateWindow>,
    append_by_stream: HashMap<String, RateWindow>,
}

impl State {
    fn rate_windows(&mut self, rejection: LoadRejection) -> &mut HashMap<String, RateWindow> {
        match rejection {
            LoadRejection::CatchUpRate => &mut self.catch_up_by_stream,
            LoadRejection::AppendRate => &mut self.append_by_stream,
            LoadRejection::PerStreamReaders | LoadRejection::PerNodeReaders => {
                unreachable!("reader limits do not use rate windows")
            }
        }
    }
}

struct RateWindow {
    started_at: Instant,
    requests: u32,
}

pub struct LiveReaderPermit {
    inner: Arc<Inner>,
    stream_key: String,
}

impl DurableStreamLoadLimiter {
    pub fn new(config: DurableStreamsLoadConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                state: Mutex::new(State::default()),
            }),
        }
    }

    pub fn try_acquire_reader(&self, stream_key: &str) -> Result<LiveReaderPermit, LoadRejection> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let stream_readers = state
            .readers_by_stream
            .get(stream_key)
            .copied()
            .unwrap_or(0);
        if stream_readers >= self.inner.config.max_concurrent_readers_per_stream {
            return reject(LoadRejection::PerStreamReaders);
        }
        if state.readers_on_node >= self.inner.config.max_concurrent_readers_per_node {
            return reject(LoadRejection::PerNodeReaders);
        }

        *state
            .readers_by_stream
            .entry(stream_key.to_owned())
            .or_default() += 1;
        state.readers_on_node += 1;
        Ok(LiveReaderPermit {
            inner: self.inner.clone(),
            stream_key: stream_key.to_owned(),
        })
    }

    pub fn check_catch_up(&self, stream_key: &str) -> Result<(), LoadRejection> {
        self.check_catch_up_at(stream_key, Instant::now())
    }

    fn check_catch_up_at(&self, stream_key: &str, now: Instant) -> Result<(), LoadRejection> {
        self.check_rate_at(
            stream_key,
            now,
            self.inner
                .config
                .max_catch_up_requests_per_second_per_stream,
            LoadRejection::CatchUpRate,
        )
    }

    pub fn check_append(&self, stream_key: &str) -> Result<(), LoadRejection> {
        self.check_append_at(stream_key, Instant::now())
    }

    fn check_append_at(&self, stream_key: &str, now: Instant) -> Result<(), LoadRejection> {
        self.check_rate_at(
            stream_key,
            now,
            self.inner.config.max_append_requests_per_second_per_stream,
            LoadRejection::AppendRate,
        )
    }

    fn check_rate_at(
        &self,
        stream_key: &str,
        now: Instant,
        limit: u32,
        rejection: LoadRejection,
    ) -> Result<(), LoadRejection> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let windows = state.rate_windows(rejection);
        windows.retain(|_, window| now.duration_since(window.started_at) < RATE_WINDOW);

        if !windows.contains_key(stream_key) && windows.len() >= MAX_TRACKED_RATE_STREAMS {
            return reject(rejection);
        }

        let window = windows.entry(stream_key.to_owned()).or_insert(RateWindow {
            started_at: now,
            requests: 0,
        });
        if window.requests >= limit {
            return reject(rejection);
        }
        window.requests += 1;
        Ok(())
    }
}

impl Drop for LiveReaderPermit {
    fn drop(&mut self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        state.readers_on_node -= 1;
        let remove = if let Some(readers) = state.readers_by_stream.get_mut(&self.stream_key) {
            *readers -= 1;
            *readers == 0
        } else {
            false
        };
        if remove {
            state.readers_by_stream.remove(&self.stream_key);
        }
    }
}

fn reject<T>(reason: LoadRejection) -> Result<T, LoadRejection> {
    crate::metrics::record_durable_stream_load_rejection(reason.metric_reason());
    Err(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn config() -> DurableStreamsLoadConfig {
        DurableStreamsLoadConfig {
            max_concurrent_readers_per_stream: 2,
            max_concurrent_readers_per_node: 3,
            max_catch_up_requests_per_second_per_stream: 2,
            max_append_requests_per_second_per_stream: 2,
        }
    }

    #[test]
    fn reader_limits_and_raii_release_are_enforced() {
        let limiter = DurableStreamLoadLimiter::new(config());
        let first = limiter.try_acquire_reader("a").unwrap();
        let second = limiter.try_acquire_reader("a").unwrap();
        assert_eq!(
            limiter.try_acquire_reader("a").err(),
            Some(LoadRejection::PerStreamReaders)
        );
        let third = limiter.try_acquire_reader("b").unwrap();
        assert_eq!(
            limiter.try_acquire_reader("c").err(),
            Some(LoadRejection::PerNodeReaders)
        );

        drop(first);
        limiter.try_acquire_reader("c").unwrap();
        drop(second);
        drop(third);
    }

    #[test]
    fn catch_up_limit_is_per_stream() {
        let limiter = DurableStreamLoadLimiter::new(config());
        let now = Instant::now();
        assert!(limiter.check_catch_up_at("a", now).is_ok());
        assert!(limiter.check_catch_up_at("a", now).is_ok());
        assert_eq!(
            limiter.check_catch_up_at("a", now),
            Err(LoadRejection::CatchUpRate)
        );
        assert!(limiter.check_catch_up_at("b", now).is_ok());
        assert!(limiter.check_catch_up_at("a", now + RATE_WINDOW).is_ok());
    }

    #[test]
    fn append_limit_expires_and_is_per_stream() {
        let limiter = DurableStreamLoadLimiter::new(config());
        let now = Instant::now();
        assert!(limiter.check_append_at("a", now).is_ok());
        assert!(limiter.check_append_at("a", now).is_ok());
        assert_eq!(
            limiter.check_append_at("a", now),
            Err(LoadRejection::AppendRate)
        );
        assert!(limiter.check_append_at("b", now).is_ok());
        assert!(limiter.check_append_at("a", now + RATE_WINDOW).is_ok());
    }

    #[test]
    fn append_limit_is_independent_from_catch_up_limit() {
        let limiter = DurableStreamLoadLimiter::new(config());
        let now = Instant::now();
        assert!(limiter.check_catch_up_at("a", now).is_ok());
        assert!(limiter.check_catch_up_at("a", now).is_ok());
        assert_eq!(
            limiter.check_catch_up_at("a", now),
            Err(LoadRejection::CatchUpRate)
        );
        assert!(limiter.check_append_at("a", now).is_ok());
        assert!(limiter.check_append_at("a", now).is_ok());
        assert_eq!(
            limiter.check_append_at("a", now),
            Err(LoadRejection::AppendRate)
        );
    }

    #[test]
    fn rejection_statuses_match_the_http_contract() {
        assert_eq!(
            LoadRejection::PerStreamReaders.status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            LoadRejection::PerNodeReaders.status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            LoadRejection::CatchUpRate.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            LoadRejection::AppendRate.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }
}
