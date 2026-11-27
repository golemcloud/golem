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

use std::time::{Duration, Instant};

use crate::services::rdbms::RdbmsType;
use golem_common::metrics::catalog;
use lazy_static::lazy_static;
use prometheus::*;

lazy_static! {
    static ref RDBMS_SUCCESS_SECONDS: HistogramVec = catalog::RDBMS_SUCCESS_SECONDS.register();
    static ref RDBMS_FAILURE_TOTAL: CounterVec = catalog::RDBMS_FAILURE_TOTAL.register();
}

pub fn record_rdbms_success<T: RdbmsType>(rdbms_type: &T, api_name: &str, duration: Duration) {
    RDBMS_SUCCESS_SECONDS
        .with_label_values(&[rdbms_type.to_string().as_str(), api_name])
        .observe(duration.as_secs_f64());
}

pub fn record_rdbms_failure<T: RdbmsType>(rdbms_type: &T, api_name: &str) {
    RDBMS_FAILURE_TOTAL
        .with_label_values(&[rdbms_type.to_string().as_str(), api_name])
        .inc();
}

pub fn record_rdbms_metrics<T: RdbmsType, R>(
    rdbms_type: &T,
    name: &'static str,
    start: Instant,
    result: std::result::Result<R, crate::services::rdbms::RdbmsError>,
) -> std::result::Result<R, crate::services::rdbms::RdbmsError> {
    let end = Instant::now();
    match result {
        Ok(result) => {
            record_rdbms_success(rdbms_type, name, end.duration_since(start));
            Ok(result)
        }
        Err(err) => {
            record_rdbms_failure(rdbms_type, name);
            Err(err)
        }
    }
}
