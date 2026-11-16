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

use crate::repo::{Deps, TestDb};
use assert2::{assert, let_assert};
use async_trait::async_trait;
use golem_common::model::account_path::{AccountPath, AccountSegment};
use golem_registry_service::repo::account_tree::AccountTreeRepo;
use golem_registry_service::repo::model::account_tree::{
    AccountLocalState, AccountNode, AccountNodeChange, AccountNodeRecord,
    AccountNodeRevisionRecord, AccountTreeRepoError, path_of_chain,
};
use golem_registry_service::repo::model::new_repo_uuid;
use golem_registry_service::repo::registry_change::{ChangeEventId, RegistryChangeEvent};
use golem_registry_service::services::account_tree_cache::AccountTreeCache;
use golem_service_base::db::{Pool, PoolApi};
use golem_service_base::repo::RepoError;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

fn segment(value: &str) -> AccountSegment {
    AccountSegment::from_str(value).unwrap()
}

/// A segment that no other test uses, padded to `len` characters.
fn unique_segment(len: usize) -> AccountSegment {
    let mut value = format!("t{}", new_repo_uuid().simple());
    while value.len() < len {
        value.push('a');
    }
    value.truncate(len);
    segment(&value)
}

fn actor() -> Uuid {
    Uuid::new_v4()
}

async fn root(deps: &Deps) -> AccountNodeRecord {
    let repo = &deps.account_tree_repo;
    if let Some(root) = repo.resolve_path(&AccountPath::root()).await.unwrap().pop() {
        return root;
    }
    match repo.create_root(Uuid::new_v4(), None, actor()).await {
        Ok(root) => root,
        Err(AccountTreeRepoError::RootExists) => repo
            .resolve_path(&AccountPath::root())
            .await
            .unwrap()
            .pop()
            .unwrap(),
        Err(err) => panic!("failed to create root: {err}"),
    }
}

async fn child(deps: &Deps, parent: Uuid, name: &AccountSegment) -> AccountNodeRecord {
    deps.account_tree_repo
        .create_child(Uuid::new_v4(), parent, name.clone(), None, actor())
        .await
        .unwrap()
}

async fn update(
    deps: &Deps,
    node: &AccountNodeRecord,
    segment: Option<AccountSegment>,
    state: AccountLocalState,
) -> Result<AccountNodeRecord, AccountTreeRepoError> {
    deps.account_tree_repo
        .update(
            node.account_id,
            node.current_revision as u64,
            AccountNodeChange {
                segment,
                display_name: node.display_name.clone(),
                state,
            },
            actor(),
        )
        .await
        .map(|signal| {
            golem_registry_service::services::registry_change_notifier::RequiresNotificationSignalExt::signal_new_events_available(
                signal,
                deps.test_registry_change_notifier().as_ref(),
            )
        })
}

async fn latest_event_id(deps: &Deps) -> ChangeEventId {
    deps.registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap_or(ChangeEventId(0))
}

async fn account_events_since(deps: &Deps, since: ChangeEventId) -> Vec<Uuid> {
    deps.registry_change_repo
        .get_events_since(since)
        .await
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            RegistryChangeEvent::AccountChanged { account_id, .. } => Some(account_id),
            _ => None,
        })
        .collect()
}

async fn history(deps: &Deps, account_id: Uuid) -> Vec<AccountNodeRevisionRecord> {
    deps.account_tree_repo
        .get_history(account_id)
        .await
        .unwrap()
}

fn path(value: &str) -> AccountPath {
    AccountPath::from_str(value).unwrap()
}

fn ids(chain: &[AccountNodeRecord]) -> Vec<Uuid> {
    chain.iter().map(|node| node.account_id).collect()
}

pub async fn test_account_tree_has_one_root(deps: &Deps) {
    let root = root(deps).await;
    let_assert!(
        Err(AccountTreeRepoError::RootExists) = deps
            .account_tree_repo
            .create_root(Uuid::new_v4(), None, actor())
            .await
    );

    let_assert!(
        Err(AccountTreeRepoError::RootImmutable) = update(
            deps,
            &root,
            Some(segment("root")),
            AccountLocalState::Active
        )
        .await
    );
    let_assert!(
        Err(AccountTreeRepoError::RootImmutable) =
            update(deps, &root, None, AccountLocalState::DeletionGrace).await
    );
    let current = deps.account_tree_repo.get(root.account_id).await.unwrap();
    assert!(current.as_ref().map(|node| node.current_revision) == Some(root.current_revision));
}

pub async fn test_account_tree_sibling_keys(deps: &Deps) {
    let root = root(deps).await;
    let top = unique_segment(20);
    let acme = child(deps, root.account_id, &top).await;
    let other = child(deps, root.account_id, &unique_segment(20)).await;

    // Concurrent creates of the same sibling: exactly one commits.
    let name = segment("team");
    let (first, second) = tokio::join!(
        deps.account_tree_repo.create_child(
            Uuid::new_v4(),
            acme.account_id,
            name.clone(),
            None,
            actor()
        ),
        deps.account_tree_repo.create_child(
            Uuid::new_v4(),
            acme.account_id,
            name.clone(),
            None,
            actor()
        ),
    );
    let outcomes = [&first, &second];
    assert!(outcomes.iter().filter(|outcome| outcome.is_ok()).count() == 1);
    assert!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Err(AccountTreeRepoError::SiblingConflict)))
            .count()
            == 1
    );

    // The same segment under a different parent is a different account.
    let elsewhere = child(deps, other.account_id, &name).await;
    let winner = first.or(second).unwrap();
    assert!(elsewhere.account_id != winner.account_id);
    let children = deps
        .account_tree_repo
        .list_children(acme.account_id)
        .await
        .unwrap();
    assert!(ids(&children) == vec![winner.account_id]);

    let_assert!(
        Err(AccountTreeRepoError::ParentNotFound) = deps
            .account_tree_repo
            .create_child(Uuid::new_v4(), Uuid::new_v4(), segment("x"), None, actor())
            .await
    );
}

pub async fn test_account_tree_failed_writes_leave_no_partial_state(deps: &Deps) {
    let root = root(deps).await;
    let acme = child(deps, root.account_id, &unique_segment(20)).await;
    let a = child(deps, acme.account_id, &segment("a")).await;
    let _b = child(deps, acme.account_id, &segment("b")).await;
    let before = latest_event_id(deps).await;

    // Renaming onto a sibling's segment conflicts and changes nothing.
    let_assert!(
        Err(AccountTreeRepoError::SiblingConflict) =
            update(deps, &a, Some(segment("b")), AccountLocalState::Active).await
    );
    // A write that names an old revision conflicts and changes nothing.
    let renamed = update(deps, &a, Some(segment("c")), AccountLocalState::Active)
        .await
        .unwrap();
    let_assert!(
        Err(AccountTreeRepoError::ConcurrentModification) =
            update(deps, &a, Some(segment("d")), AccountLocalState::Active).await
    );

    let current = deps
        .account_tree_repo
        .get(a.account_id)
        .await
        .unwrap()
        .unwrap();
    assert!(current == renamed);
    let revisions = history(deps, a.account_id).await;
    assert!(revisions.iter().map(|r| r.revision).collect::<Vec<_>>() == vec![0, 1]);
    assert!(revisions[1].segment == "c");
    assert!(revisions[1].segment == current.segment);
    // Only the committed rename journaled an event.
    assert!(account_events_since(deps, before).await == vec![a.account_id]);
}

pub async fn test_account_tree_parent_is_immutable(deps: &Deps) {
    let root = root(deps).await;
    let left = child(deps, root.account_id, &unique_segment(20)).await;
    let right = child(deps, root.account_id, &unique_segment(20)).await;
    let leaf = child(deps, left.account_id, &segment("leaf")).await;

    let result = match &deps.test_db {
        TestDb::Postgres(pool) => pool
            .with_rw("account_tree_test", "move_parent")
            .execute(
                sqlx::query("UPDATE account_nodes SET parent_id = $1 WHERE account_id = $2")
                    .bind(right.account_id)
                    .bind(leaf.account_id),
            )
            .await
            .map(|_| ()),
        TestDb::Sqlite(pool) => pool
            .with_rw("account_tree_test", "move_parent")
            .execute(
                sqlx::query("UPDATE account_nodes SET parent_id = $1 WHERE account_id = $2")
                    .bind(right.account_id)
                    .bind(leaf.account_id),
            )
            .await
            .map(|_| ()),
    };
    let_assert!(Err(RepoError::InternalError(_)) = result);
    let current = deps
        .account_tree_repo
        .get(leaf.account_id)
        .await
        .unwrap()
        .unwrap();
    assert!(current.parent_id == Some(left.account_id));
}

pub async fn test_account_tree_maximum_bounds_and_rename(deps: &Deps) {
    let root = root(deps).await;
    let mut chain = vec![root.clone()];
    let mut text = String::new();
    for level in 0..AccountPath::MAX_DEPTH {
        let name = if level == 0 {
            unique_segment(AccountSegment::MAX_LEN)
        } else {
            segment(&"s".repeat(AccountSegment::MAX_LEN))
        };
        text.push('/');
        text.push_str(name.as_str());
        let node = child(deps, chain.last().unwrap().account_id, &name).await;
        chain.push(node);
    }
    assert!(text.len() == AccountPath::MAX_LEN);
    let full = path(&text);
    let resolved = deps.account_tree_repo.resolve_path(&full).await.unwrap();
    assert!(ids(&resolved) == ids(&chain));

    let leaf = chain.last().unwrap();
    let_assert!(
        Err(AccountTreeRepoError::TooDeep) = deps
            .account_tree_repo
            .create_child(Uuid::new_v4(), leaf.account_id, segment("x"), None, actor())
            .await
    );

    // Renaming the top segment to another maximum-length one keeps every ID and revision below
    // it; no descendant is rewritten.
    let new_top = unique_segment(AccountSegment::MAX_LEN);
    update(
        deps,
        &chain[1],
        Some(new_top.clone()),
        AccountLocalState::Active,
    )
    .await
    .unwrap();
    let renamed_text = format!(
        "/{}{}",
        new_top.as_str(),
        &text[1 + AccountSegment::MAX_LEN..]
    );
    assert!(renamed_text.len() == AccountPath::MAX_LEN);
    let renamed = deps
        .account_tree_repo
        .resolve_path(&path(&renamed_text))
        .await
        .unwrap();
    assert!(ids(&renamed) == ids(&chain));
    for (before, after) in chain.iter().zip(&renamed).skip(2) {
        assert!(before.current_revision == after.current_revision);
    }
    assert!(
        deps.account_tree_repo
            .resolve_path(&full)
            .await
            .unwrap()
            .is_empty()
    );

    let ancestry = deps
        .account_tree_repo
        .get_ancestry(leaf.account_id)
        .await
        .unwrap();
    let nodes: Vec<AccountNode> = ancestry
        .into_iter()
        .map(|record| AccountNode::try_from(record).unwrap())
        .collect();
    assert!(path_of_chain(&nodes).unwrap() == path(&renamed_text));
}

pub async fn test_account_tree_grace_reserves_name_until_purge(deps: &Deps) {
    let root = root(deps).await;
    let acme = child(deps, root.account_id, &unique_segment(20)).await;
    let name = segment("customer-1");
    let customer = child(deps, acme.account_id, &name).await;
    let nested = child(deps, customer.account_id, &segment("nested")).await;

    let_assert!(
        Err(AccountTreeRepoError::NotInDeletionGrace) = deps
            .account_tree_repo
            .purge(
                customer.account_id,
                customer.current_revision as u64,
                actor()
            )
            .await
    );

    let customer = update(deps, &customer, None, AccountLocalState::DeletionGrace)
        .await
        .unwrap();
    let_assert!(
        Err(AccountTreeRepoError::SiblingConflict) = deps
            .account_tree_repo
            .create_child(Uuid::new_v4(), acme.account_id, name.clone(), None, actor())
            .await
    );
    // The grace node is still resolvable; callers read its lifecycle state.
    let resolved = deps
        .account_tree_repo
        .resolve_path(&path(&format!("/{}/customer-1", acme.segment)))
        .await
        .unwrap();
    assert!(resolved.last().map(|node| node.account_id) == Some(customer.account_id));

    let_assert!(
        Err(AccountTreeRepoError::HasChildren) = deps
            .account_tree_repo
            .purge(
                customer.account_id,
                customer.current_revision as u64,
                actor()
            )
            .await
    );
    let nested = update(deps, &nested, None, AccountLocalState::DeletionGrace)
        .await
        .unwrap();
    deps.account_tree_repo
        .purge(nested.account_id, nested.current_revision as u64, actor())
        .await
        .unwrap()
        .signal_new_events_available_for_test(deps);
    deps.account_tree_repo
        .purge(
            customer.account_id,
            customer.current_revision as u64,
            actor(),
        )
        .await
        .unwrap()
        .signal_new_events_available_for_test(deps);

    let reused = child(deps, acme.account_id, &name).await;
    assert!(reused.account_id != customer.account_id);
    assert!(
        deps.account_tree_repo
            .get(customer.account_id)
            .await
            .unwrap()
            .is_none()
    );
    // History outlives the purge.
    assert!(history(deps, customer.account_id).await.len() == 2);
}

trait SignalForTest {
    fn signal_new_events_available_for_test(self, deps: &Deps);
}

impl<T> SignalForTest
    for golem_registry_service::repo::registry_change::RequiresNotificationSignal<T>
{
    fn signal_new_events_available_for_test(self, deps: &Deps) {
        golem_registry_service::services::registry_change_notifier::RequiresNotificationSignalExt::signal_new_events_available(
            self,
            deps.test_registry_change_notifier().as_ref(),
        );
    }
}

/// Delegates to the real repository, counts reads and can hold an ancestry load after it has
/// read the database.
struct ObservedRepo {
    inner: Arc<dyn AccountTreeRepo>,
    reads: AtomicUsize,
    hold: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl ObservedRepo {
    fn new(inner: Arc<dyn AccountTreeRepo>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            reads: AtomicUsize::new(0),
            hold: AtomicBool::new(false),
            entered: Notify::new(),
            release: Notify::new(),
        })
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AccountTreeRepo for ObservedRepo {
    async fn create_root(
        &self,
        account_id: Uuid,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError> {
        self.inner
            .create_root(account_id, display_name, actor)
            .await
    }

    async fn create_child(
        &self,
        account_id: Uuid,
        parent_id: Uuid,
        segment: AccountSegment,
        display_name: Option<String>,
        actor: Uuid,
    ) -> Result<AccountNodeRecord, AccountTreeRepoError> {
        self.inner
            .create_child(account_id, parent_id, segment, display_name, actor)
            .await
    }

    async fn update(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        change: AccountNodeChange,
        actor: Uuid,
    ) -> Result<
        golem_registry_service::repo::registry_change::RequiresNotificationSignal<
            AccountNodeRecord,
        >,
        AccountTreeRepoError,
    > {
        self.inner
            .update(account_id, expected_revision, change, actor)
            .await
    }

    async fn purge(
        &self,
        account_id: Uuid,
        expected_revision: u64,
        actor: Uuid,
    ) -> Result<
        golem_registry_service::repo::registry_change::RequiresNotificationSignal<()>,
        AccountTreeRepoError,
    > {
        self.inner.purge(account_id, expected_revision, actor).await
    }

    async fn get(&self, account_id: Uuid) -> Result<Option<AccountNodeRecord>, RepoError> {
        self.inner.get(account_id).await
    }

    async fn get_ancestry(&self, account_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.get_ancestry(account_id).await;
        if self.hold.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
    }

    async fn resolve_path(&self, path: &AccountPath) -> Result<Vec<AccountNodeRecord>, RepoError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve_path(path).await
    }

    async fn list_children(&self, parent_id: Uuid) -> Result<Vec<AccountNodeRecord>, RepoError> {
        self.inner.list_children(parent_id).await
    }

    async fn get_history(
        &self,
        account_id: Uuid,
    ) -> Result<Vec<AccountNodeRevisionRecord>, RepoError> {
        self.inner.get_history(account_id).await
    }
}

/// Delivers the journal's account events since `since` to the cache, as the notifier does.
async fn deliver_events(deps: &Deps, cache: &AccountTreeCache, since: ChangeEventId) {
    for event in deps
        .registry_change_repo
        .get_events_since(since)
        .await
        .unwrap()
    {
        cache.handle_event(&event);
    }
}

fn chain_path(chain: &[AccountNode]) -> AccountPath {
    path_of_chain(chain).unwrap()
}

pub async fn test_account_tree_cache_follows_ancestor_changes(deps: &Deps) {
    let root = root(deps).await;
    let top = unique_segment(20);
    let acme = child(deps, root.account_id, &top).await;
    let team = child(deps, acme.account_id, &segment("team")).await;
    let leaf = child(deps, team.account_id, &segment("leaf")).await;

    let repo = ObservedRepo::new(deps.account_tree_repo.clone());
    let cache = AccountTreeCache::new(repo.clone(), 64);
    let leaf_path = path(&format!("/{}/team/leaf", top.as_str()));

    let cold = cache.ancestry(leaf.account_id).await.unwrap().unwrap();
    assert!(chain_path(&cold) == leaf_path);
    assert!(repo.reads() == 1);
    // Warm reads by ID and by path make no database call.
    let warm = cache.ancestry(leaf.account_id).await.unwrap().unwrap();
    let by_path = cache.resolve(&leaf_path).await.unwrap().unwrap();
    assert!(warm == cold);
    assert!(by_path == cold);
    assert!(repo.reads() == 1);

    // Ancestor-only changes: the leaf's own revision stays, its path and effective state change.
    let since = latest_event_id(deps).await;
    let team = update(
        deps,
        &team,
        Some(segment("renamed")),
        AccountLocalState::Active,
    )
    .await
    .unwrap();
    update(deps, &acme, None, AccountLocalState::Suspended)
        .await
        .unwrap();
    deliver_events(deps, &cache, since).await;

    let fresh = cache.ancestry(leaf.account_id).await.unwrap().unwrap();
    assert!(chain_path(&fresh) == path(&format!("/{}/renamed/leaf", top.as_str())));
    assert!(AccountLocalState::effective(&fresh) == AccountLocalState::Suspended);
    assert!(fresh.last().unwrap().revision == leaf.current_revision as u64);
    assert!(fresh[2].revision == team.current_revision as u64);
    assert!(cache.resolve(&leaf_path).await.unwrap().is_none());
}

pub async fn test_account_tree_cache_rejects_delayed_load(deps: &Deps) {
    let root = root(deps).await;
    let top = unique_segment(20);
    let acme = child(deps, root.account_id, &top).await;
    let leaf = child(deps, acme.account_id, &segment("leaf")).await;

    let repo = ObservedRepo::new(deps.account_tree_repo.clone());
    let cache = Arc::new(AccountTreeCache::new(repo.clone(), 64));

    // The load reads the old ancestry, then stalls before it reaches the cache.
    repo.hold.store(true, Ordering::SeqCst);
    let entered = repo.entered.notified();
    let load = tokio::spawn({
        let cache = cache.clone();
        async move { cache.ancestry(leaf.account_id).await.unwrap().unwrap() }
    });
    entered.await;

    let since = latest_event_id(deps).await;
    update(deps, &acme, None, AccountLocalState::Suspended)
        .await
        .unwrap();
    deliver_events(deps, &cache, since).await;
    repo.release.notify_one();

    // The racing caller sees the value it read; the cache does not keep it.
    let delayed = load.await.unwrap();
    assert!(AccountLocalState::effective(&delayed) == AccountLocalState::Active);
    assert!(cache.is_empty());
    let fresh = cache.ancestry(leaf.account_id).await.unwrap().unwrap();
    assert!(AccountLocalState::effective(&fresh) == AccountLocalState::Suspended);
}

pub async fn test_account_tree_cache_eviction_and_lag(deps: &Deps) {
    let root = root(deps).await;
    let acme = child(deps, root.account_id, &unique_segment(20)).await;
    let mut leaves = Vec::new();
    for index in 0..12 {
        leaves.push(child(deps, acme.account_id, &segment(&format!("c{index}"))).await);
    }

    let repo = ObservedRepo::new(deps.account_tree_repo.clone());
    let capacity = AccountPath::MAX_DEPTH + 1;
    let cache = Arc::new(AccountTreeCache::new(repo.clone(), capacity));
    for leaf in &leaves {
        cache.ancestry(leaf.account_id).await.unwrap().unwrap();
    }
    assert!(cache.len() <= capacity);

    // The first leaf was evicted; a change while it is out of the cache is read on reload.
    let first = &leaves[0];
    update(
        deps,
        first,
        Some(segment("moved")),
        AccountLocalState::Active,
    )
    .await
    .unwrap();
    let reads = repo.reads();
    let reloaded = cache.ancestry(first.account_id).await.unwrap().unwrap();
    assert!(repo.reads() == reads + 1);
    assert!(reloaded.last().unwrap().segment == Some(segment("moved")));

    // A lagged subscription may have dropped an account event, so the cache clears.
    let last = leaves.last().unwrap();
    cache.ancestry(last.account_id).await.unwrap().unwrap();
    update(deps, last, None, AccountLocalState::Suspended)
        .await
        .unwrap();
    let (sender, receiver) = tokio::sync::broadcast::channel(1);
    for event_id in 0..3 {
        let _ = sender.send(RegistryChangeEvent::AccountChanged {
            event_id: ChangeEventId(event_id),
            account_id: Uuid::new_v4(),
        });
    }
    let invalidation = tokio::spawn({
        let cache = cache.clone();
        async move { cache.run_invalidation(receiver).await }
    });
    let cleared = tokio::time::timeout(Duration::from_secs(10), async {
        while !cache.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(cleared.is_ok());
    let fresh = cache.ancestry(last.account_id).await.unwrap().unwrap();
    assert!(AccountLocalState::effective(&fresh) == AccountLocalState::Suspended);
    drop(sender);
    invalidation.await.unwrap();
}
