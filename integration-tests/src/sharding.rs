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

use anyhow::{Context, ensure};
use std::future::Future;

/// Observes the expected count on both earlier executors in one scrape batch.
pub async fn observe_executor_count<F, Fut>(
    expected: usize,
    mut scrape: F,
) -> anyhow::Result<Option<()>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Box<[String]>>>,
{
    let metrics = scrape().await?;
    ensure!(
        metrics.len() == 2,
        "expected metrics from both earlier executors"
    );
    let ready = metrics.iter().try_fold(true, |ready, metrics| {
        let count = sample(metrics, "executor_count")?;
        anyhow::Ok(ready && count == expected as u64)
    })?;
    Ok(ready.then_some(()))
}

fn sample(metrics: &str, name: &str) -> anyhow::Result<u64> {
    metrics
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .with_context(|| format!("missing required metric {name}"))?
        .parse()
        .with_context(|| format!("invalid metric {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;
    use test_r::test;

    #[test]
    async fn both_executor_counts_are_observed_without_a_delivery_claim() {
        futures::future::join_all(
            [([3, 3], Some(())), ([2, 3], None), ([3, 2], None)]
                .into_iter()
                .map(|(counts, expected)| async move {
                    let observed = observe_executor_count(3, || {
                        ready(Ok(
                            Box::new(counts.map(|count| format!("executor_count {count}\n")))
                                as Box<[String]>,
                        ))
                    })
                    .await
                    .unwrap();
                    assert_eq!(observed, expected);
                }),
        )
        .await;
    }

    #[test]
    async fn missing_or_invalid_counts_are_errors_on_either_executor() {
        futures::future::join_all(
            ["", "executor_count invalid\n"]
                .into_iter()
                .flat_map(|metrics| [0, 1].map(move |executor| (metrics, executor)))
                .map(|(metrics, executor)| async move {
                    let observed = observe_executor_count(3, || {
                        let mut batch =
                            std::array::from_fn::<_, 2, _>(|_| "executor_count 3\n".to_owned());
                        batch[executor] = metrics.to_owned();
                        ready(Ok(Box::new(batch) as Box<[String]>))
                    })
                    .await;
                    assert!(observed.is_err());
                }),
        )
        .await;
    }
}
