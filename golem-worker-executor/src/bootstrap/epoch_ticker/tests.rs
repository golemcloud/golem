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
use test_r::test;

#[test]
#[test_r::timeout("10s")]
async fn an_early_bootstrap_error_joins_the_epoch_thread_and_releases_its_engine() {
    tokio::task::spawn_blocking(|| {
        let engine = Arc::new(());
        let released = Arc::downgrade(&engine);
        let failed: Result<(), &'static str> = {
            let _ticker = EpochTicker::start(Duration::from_secs(60), move || {
                let _ = &engine;
            });
            Err("snapshot binding failed")
        };
        assert!(failed.is_err());
        assert!(released.upgrade().is_none());
    })
    .await
    .unwrap();
}
