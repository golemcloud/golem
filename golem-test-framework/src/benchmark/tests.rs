// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use test_r::test;

use crate::benchmark::config::RunConfig;
use crate::benchmark::{
    BenchmarkResult, BenchmarkRunResult, CountResult, DurationResult, ResultKey,
};
use std::collections::HashMap;
use std::time::Duration;

#[test]
fn benchmark_result_is_serializable_to_json() {
    let rc1 = RunConfig {
        cluster_size: 1,
        size: 10,
        length: 20,
    };
    let rc2 = RunConfig {
        cluster_size: 5,
        size: 100,
        length: 20,
    };

    let mut dr1 = HashMap::new();
    let mut cr1 = HashMap::new();
    let mut dr2 = HashMap::new();
    let mut cr2 = HashMap::new();

    dr1.insert(
        ResultKey::primary("a"),
        DurationResult {
            avg: Duration::from_millis(500),
            min: Duration::from_millis(100),
            max: Duration::from_millis(900),
            all: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_millis(900),
            ],
            per_iteration: vec![vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_millis(900),
            ]],
        },
    );

    cr1.insert(
        ResultKey::primary("a"),
        CountResult {
            avg: 500,
            min: 100,
            max: 900,
            all: vec![100, 500, 900],
            per_iteration: vec![vec![100, 500, 900]],
        },
    );

    dr2.insert(
        ResultKey::secondary("b"),
        DurationResult {
            avg: Duration::from_millis(500),
            min: Duration::from_millis(100),
            max: Duration::from_millis(900),
            all: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_millis(900),
            ],
            per_iteration: vec![vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_millis(900),
            ]],
        },
    );

    cr2.insert(
        ResultKey::secondary("b"),
        CountResult {
            avg: 500,
            min: 100,
            max: 900,
            all: vec![100, 500, 900],
            per_iteration: vec![vec![100, 500, 900]],
        },
    );

    let example = BenchmarkResult {
        runs: vec![rc1.clone(), rc2.clone()],
        results: vec![
            (
                rc1,
                BenchmarkRunResult {
                    duration_results: dr1,
                    count_results: cr1,
                },
            ),
            (
                rc2,
                BenchmarkRunResult {
                    duration_results: dr2,
                    count_results: cr2,
                },
            ),
        ],
    };

    let json = serde_json::to_string_pretty(&example).unwrap();
    assert_eq!(
        json,
        r#"{
  "runs": [
    {
      "cluster_size": 1,
      "size": 10,
      "length": 20
    },
    {
      "cluster_size": 5,
      "size": 100,
      "length": 20
    }
  ],
  "results": [
    [
      {
        "cluster_size": 1,
        "size": 10,
        "length": 20
      },
      {
        "duration_results": {
          "a": {
            "avg": {
              "secs": 0,
              "nanos": 500000000
            },
            "min": {
              "secs": 0,
              "nanos": 100000000
            },
            "max": {
              "secs": 0,
              "nanos": 900000000
            },
            "all": [
              {
                "secs": 0,
                "nanos": 100000000
              },
              {
                "secs": 0,
                "nanos": 500000000
              },
              {
                "secs": 0,
                "nanos": 900000000
              }
            ],
            "per_iteration": [
              [
                {
                  "secs": 0,
                  "nanos": 100000000
                },
                {
                  "secs": 0,
                  "nanos": 500000000
                },
                {
                  "secs": 0,
                  "nanos": 900000000
                }
              ]
            ]
          }
        },
        "count_results": {
          "a": {
            "avg": 500,
            "min": 100,
            "max": 900,
            "all": [
              100,
              500,
              900
            ],
            "per_iteration": [
              [
                100,
                500,
                900
              ]
            ]
          }
        }
      }
    ],
    [
      {
        "cluster_size": 5,
        "size": 100,
        "length": 20
      },
      {
        "duration_results": {
          "b__secondary": {
            "avg": {
              "secs": 0,
              "nanos": 500000000
            },
            "min": {
              "secs": 0,
              "nanos": 100000000
            },
            "max": {
              "secs": 0,
              "nanos": 900000000
            },
            "all": [
              {
                "secs": 0,
                "nanos": 100000000
              },
              {
                "secs": 0,
                "nanos": 500000000
              },
              {
                "secs": 0,
                "nanos": 900000000
              }
            ],
            "per_iteration": [
              [
                {
                  "secs": 0,
                  "nanos": 100000000
                },
                {
                  "secs": 0,
                  "nanos": 500000000
                },
                {
                  "secs": 0,
                  "nanos": 900000000
                }
              ]
            ]
          }
        },
        "count_results": {
          "b__secondary": {
            "avg": 500,
            "min": 100,
            "max": 900,
            "all": [
              100,
              500,
              900
            ],
            "per_iteration": [
              [
                100,
                500,
                900
              ]
            ]
          }
        }
      }
    ]
  ]
}"#
    );

    let deserialized = serde_json::from_str::<BenchmarkResult>(&json).unwrap();
    assert_eq!(example, deserialized);
}
