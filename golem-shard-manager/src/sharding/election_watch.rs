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

//! A replica's view of the leader election, kept current by an etcd watch on the election prefix.
//!
//! A standby answers each refused call from this view instead of reading etcd for each call, so
//! that an etcd outage does not stall every call that reaches a standby. A stale view is
//! harmless there: the client follows the named leader and comes back if it is gone. The
//! campaign uses the view to find a duplicate advertised address and a campaigner of a newer
//! version, and the readiness check uses the time of etcd's last answer.

use crate::sharding::error::{ShardManagerError, is_stale_auth_token, is_stale_auth_token_reason};
use crate::sharding::leader_election::{ElectionValue, RetryDelays, bounded};
use etcd_client::{Client, GetOptions, KeyValue, KvClient, WatchOptions};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior, interval, sleep};
use tracing::warn;

#[derive(Clone, Debug, PartialEq)]
pub struct Campaigner {
    pub key: Vec<u8>,
    /// The lease the key is attached to: how a replica finds its own key.
    pub lease: i64,
    pub create_revision: i64,
    /// `None` when the value cannot be read, as a value that is not a JSON `ElectionValue`.
    pub value: Option<ElectionValue>,
}

/// What a replica knows of the leader.
#[derive(Clone, Debug, PartialEq)]
pub enum LeaderView {
    /// Etcd has not answered yet.
    Unread,
    /// No replica leads, or the leader's value is not a readable `ElectionValue`; the two are
    /// treated alike.
    NoLeader,
    Leader(ElectionValue),
}

/// Every campaigner, in queue order: etcd hands the leadership to the key with the lowest create
/// revision.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CampaignView {
    pub campaigners: Vec<Campaigner>,
}

impl CampaignView {
    pub fn from_kvs(kvs: &[KeyValue]) -> Self {
        Self::from_entries(kvs.iter().map(|kv| {
            (
                kv.key().to_vec(),
                kv.lease(),
                kv.create_revision(),
                kv.value().to_vec(),
            )
        }))
    }

    fn from_entries(entries: impl IntoIterator<Item = (Vec<u8>, i64, i64, Vec<u8>)>) -> Self {
        let mut campaigners: Vec<Campaigner> = entries
            .into_iter()
            .map(|(key, lease, create_revision, value)| Campaigner {
                key,
                lease,
                create_revision,
                value: ElectionValue::decode(&value),
            })
            .collect();
        campaigners.sort_by_key(|campaigner| campaigner.create_revision);
        Self { campaigners }
    }

    pub fn leader(&self) -> Option<&Campaigner> {
        self.campaigners.first()
    }
}

/// Read side, cloned wherever the view is needed.
#[derive(Clone)]
pub struct ElectionWatch {
    view: watch::Receiver<Option<CampaignView>>,
    last_answer: Arc<Mutex<Option<Instant>>>,
    restarts: Arc<AtomicU64>,
}

impl ElectionWatch {
    /// `None` until etcd has answered once.
    pub fn view(&self) -> Option<CampaignView> {
        self.view.borrow().clone()
    }

    /// What this replica currently knows of the leader.
    pub fn leader_value(&self) -> LeaderView {
        match self.view.borrow().as_ref() {
            None => LeaderView::Unread,
            Some(view) => match view.leader().and_then(|leader| leader.value.clone()) {
                Some(value) => LeaderView::Leader(value),
                None => LeaderView::NoLeader,
            },
        }
    }

    /// Whether etcd answered this replica's watch within `window`.
    pub fn answered_within(&self, window: Duration) -> bool {
        answered_within(&self.last_answer, window)
    }

    /// How many times the watcher has given up its stream and read the election again.
    ///
    /// **Tests only**: it lets a test tell a watch that recovered by restarting from one that
    /// merely received delayed bytes.
    #[doc(hidden)]
    pub fn restarts(&self) -> u64 {
        self.restarts.load(Ordering::SeqCst)
    }

    /// Waits for the next change of the view. Pends forever once the watcher has stopped, which
    /// only happens when the replica stops.
    pub async fn changed(&mut self) {
        if self.view.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Write side: the task that keeps the view current. Runs until the replica stops.
pub struct ElectionWatcher {
    client: Client,
    prefix: String,
    request_timeout: Duration,
    /// Past this much silence from etcd the watch is started again: a black-holed connection
    /// yields neither an error nor a close.
    quiet_limit: Duration,
    retry: RetryDelays,
    view: watch::Sender<Option<CampaignView>>,
    last_answer: Arc<Mutex<Option<Instant>>>,
    restarts: Arc<AtomicU64>,
}

impl ElectionWatcher {
    pub fn new(
        client: Client,
        election_name: &str,
        request_timeout: Duration,
        quiet_limit: Duration,
        retry: RetryDelays,
    ) -> (ElectionWatch, Self) {
        let (sender, receiver) = watch::channel(None);
        let last_answer = Arc::new(Mutex::new(None));
        let restarts = Arc::new(AtomicU64::new(0));
        (
            ElectionWatch {
                view: receiver,
                last_answer: last_answer.clone(),
                restarts: restarts.clone(),
            },
            Self {
                client,
                prefix: format!("{election_name}/"),
                request_timeout,
                quiet_limit,
                retry,
                view: sender,
                last_answer,
                restarts,
            },
        )
    }

    /// Follows the election until the replica stops. A failure keeps the last view: it is still
    /// the best answer this replica has.
    ///
    /// etcd checks the token when the watcher reads the election and when it creates the watch,
    /// but not for the events and progress answers of a watch it has created, so a change to
    /// etcd's users or roles shows only at the next read: after the next change of the election,
    /// or after a silence. etcd then refuses a JWT token from before the change, and the watcher
    /// logs in again and follows the election again at once; otherwise it would keep the view
    /// from before the change for as long as it runs.
    pub async fn run(self) -> anyhow::Result<()> {
        let mut backoff = self.retry.min;
        loop {
            let err = self.follow(&mut backoff).await;
            self.restarts.fetch_add(1, Ordering::SeqCst);
            if is_stale_auth_token(&err) {
                warn!(
                    error = %err,
                    "etcd's users or roles changed since this replica logged in; logging in again \
                     to watch the shard manager election"
                );
                match bounded(self.request_timeout, self.client.refresh_token()).await {
                    Ok(()) => continue,
                    Err(login_failed) => warn!(
                        error = %login_failed,
                        "Cannot log in to etcd again to watch the shard manager election"
                    ),
                }
            }
            warn!(
                error = %err,
                retry_in = ?backoff,
                "Lost the watch on the shard manager election; reading it again"
            );
            sleep(backoff).await;
            backoff = std::cmp::min(backoff.saturating_mul(2), self.retry.max);
        }
    }

    /// Reads the campaign, then watches it from the next revision. Returns only on a failure.
    async fn follow(&self, backoff: &mut Duration) -> ShardManagerError {
        let mut kv = self.client.kv_client();
        let revision = match self.read(&mut kv).await {
            Ok(revision) => revision,
            Err(err) => return err,
        };

        let mut watch_client = self.client.watch_client();
        let options = WatchOptions::new()
            .with_prefix()
            .with_start_revision(revision + 1);
        let stream = match bounded(
            self.request_timeout,
            watch_client.watch(self.prefix.clone(), Some(options)),
        )
        .await
        {
            Ok(stream) => stream,
            Err(err) => return err,
        };
        let (mut requests, mut responses) = stream.split();

        // Progress requests make etcd answer a quiet watch, so silence means trouble.
        let mut progress = interval(self.quiet_limit / 3);
        progress.set_missed_tick_behavior(MissedTickBehavior::Delay);
        progress.tick().await;

        // A read alone proves little: a watch that fails right after it is created would
        // otherwise re-read at the shortest delay for ever. The backoff starts over only once
        // the stream has delivered a message.
        let mut delivered = false;
        loop {
            tokio::select! {
                message = responses.message() => match message {
                    Err(err) => return err.into(),
                    Ok(None) => {
                        return ShardManagerError::Internal(
                            "the etcd watch on the election ended".to_string(),
                        );
                    }
                    // etcd refuses to create a watch with a token from before a change to its
                    // users or roles by cancelling it at once, with the refusal as the reason.
                    Ok(Some(response))
                        if response.canceled()
                            && is_stale_auth_token_reason(response.cancel_reason()) =>
                    {
                        return ShardManagerError::EtcdError(etcd_client::Error::WatchError(
                            response.cancel_reason().to_string(),
                        ));
                    }
                    // A compaction past the watch's revision cancels it; reading again resumes.
                    Ok(Some(response)) if response.canceled() => {
                        return ShardManagerError::Internal(format!(
                            "etcd cancelled the watch on the election (compacted to revision \
                             {}): {}",
                            response.compact_revision(),
                            response.cancel_reason()
                        ));
                    }
                    Ok(Some(response)) => {
                        if !delivered {
                            delivered = true;
                            *backoff = self.retry.min;
                        }
                        self.heard();
                        if !response.events().is_empty()
                            && let Err(err) = self.read(&mut kv).await
                        {
                            return err;
                        }
                    }
                },
                _ = progress.tick() => {
                    if !answered_within(&self.last_answer, self.quiet_limit) {
                        return ShardManagerError::Internal(format!(
                            "etcd has not answered the election watch for {:?}",
                            self.quiet_limit
                        ));
                    }
                    if let Err(err) = requests.request_progress().await {
                        return err.into();
                    }
                }
            }
        }
    }

    /// Reads every campaigner and publishes the view. Returns the revision it was read at.
    async fn read(&self, kv: &mut KvClient) -> Result<i64, ShardManagerError> {
        let response = bounded(
            self.request_timeout,
            kv.get(self.prefix.clone(), Some(GetOptions::new().with_prefix())),
        )
        .await?;
        self.heard();
        self.view
            .send_replace(Some(CampaignView::from_kvs(response.kvs())));
        Ok(response
            .header()
            .map(|header| header.revision())
            .unwrap_or_default())
    }

    fn heard(&self) {
        *self
            .last_answer
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }
}

fn answered_within(last_answer: &Mutex<Option<Instant>>, window: Duration) -> bool {
    last_answer
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some_and(|at| at.elapsed() < window)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_service_base::clients::shard_manager_leader::LeaderAddress;
    use test_r::test;

    fn value(port: u16, ready: bool) -> Vec<u8> {
        ElectionValue {
            identity: format!("sm-{port}"),
            grpc_address: LeaderAddress {
                host: "10.0.0.1".to_string(),
                port,
            },
            ready,
            version: String::new(),
        }
        .encode()
    }

    #[test]
    fn the_leader_is_the_campaigner_with_the_lowest_create_revision() {
        let view = CampaignView::from_entries([
            (b"e/2".to_vec(), 2, 20, value(9002, false)),
            (b"e/1".to_vec(), 1, 10, value(9001, true)),
        ]);
        assert_eq!(view.leader().unwrap().lease, 1);
        assert_eq!(view.campaigners.len(), 2);
    }

    #[test]
    fn a_value_that_is_not_json_reads_as_none() {
        // A value from a replica that wrote only its identity.
        let view =
            CampaignView::from_entries([(b"e/1".to_vec(), 1, 10, b"sm-old/pid-1/17".to_vec())]);
        assert_eq!(view.leader().unwrap().value, None);
    }
}
