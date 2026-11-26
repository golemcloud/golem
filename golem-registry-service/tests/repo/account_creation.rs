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

//! A test-only composition of child-account creation: the tree node, the initial plan
//! assignment, fixture Cards records and the configuration event commit in one transaction.
//! The fixture Cards table stands in for the real Cards participant and exists only in tests.

use crate::repo::account_tree::{
    SignalForTest, account_events_since, child, latest_event_id, root, unique_segment, update,
};
use crate::repo::plan_storage::{define, edit, hold_guard, range, table_bounds};
use crate::repo::{Deps, TestDb};
use assert2::{assert, let_assert};
use conditional_trait_gen::trait_gen;
use futures::FutureExt;
use golem_common::base_model::caller::{AccountAuthentication, AccountPrincipal, Pair};
use golem_common::model::account::AccountId;
use golem_common::model::account_path::AccountSegment;
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::model::plan_dimension::{DeclaredMax, EffectiveBounds, EndpointSource};
use golem_registry_service::repo::account_guard::{
    DbGuards, GuardClass, GuardMode, GuardSet, is_guard_timeout,
};
use golem_registry_service::repo::account_tree::DbAccountTreeRepo;
use golem_registry_service::repo::model::account_tree::{
    AccountLifecycleState, AccountNodeRecord, AccountTreeRepoError,
};
use golem_registry_service::repo::model::plan_storage::{
    PlanAssignmentRecord, PlanStorageRepoError,
};
use golem_registry_service::repo::plan_storage::DbPlanStorageRepo;
use golem_registry_service::repo::registry_change::{
    DbRegistryChangeRepo, NewRegistryChangeEvent, RegistryChangeEvent, RegistryChangeRepo,
    RequiresNotificationSignal, RequiresSignalExt,
};
use golem_registry_service::services::registry_change_notifier::{
    PostgresRegistryChangeNotifier, RegistryChangeNotifier, RequiresNotificationSignalExt,
    SqliteRegistryChangeNotifier,
};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{LabelledPoolTransaction, Pool, PoolApi};
use golem_service_base::repo::RepoError;
use std::marker::PhantomData;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Barrier, broadcast};
use uuid::Uuid;

const FIXTURE_CARDS_TABLE: &str = "CREATE TABLE IF NOT EXISTS account_creation_fixture_cards \
     (card_id UUID NOT NULL PRIMARY KEY, \
      wallet_account_id UUID NOT NULL REFERENCES account_nodes (account_id), \
      holder_issuer TEXT NOT NULL, \
      holder_subject TEXT NOT NULL, \
      effect TEXT NOT NULL)";

/// Serializes the fixture table creation of concurrent tests in this process.
static FIXTURE_CARDS_TABLE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The fixture Cards effects of a new account: its wallet and the root card in that wallet.
const FIXTURE_EFFECTS: [&str; 2] = ["root", "wallet"];

const GUARD_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: usize = 5;

/// One child creation. The account ID and its pair are allocated once and kept by every
/// attempt.
#[derive(Clone)]
struct NewChild {
    account_id: Uuid,
    pair: Pair,
    parent_id: Uuid,
    segment: AccountSegment,
    plan_id: Uuid,
    actor: Uuid,
}

impl NewChild {
    async fn new(deps: &Deps, parent_id: Uuid, segment: AccountSegment, plan_id: Uuid) -> Self {
        let account_id = Uuid::new_v4();
        Self {
            account_id,
            pair: account_pair(deps, account_id).await,
            parent_id,
            segment,
            plan_id,
            actor: Uuid::new_v4(),
        }
    }
}

/// The installation's Golem issuer, or a generated one when the shared test database has no
/// stored issuer. The tests do not store one, because another test checks the first store.
async fn installation_issuer(deps: &Deps) -> String {
    match deps.installation_repo.get().await.unwrap() {
        Some(installation) => installation.golem_issuer,
        None => GolemIssuer::generate().to_string(),
    }
}

async fn account_pair(deps: &Deps, account_id: Uuid) -> Pair {
    AccountPrincipal {
        issuer: installation_issuer(deps).await,
        account_id: AccountId(account_id),
        authentication: AccountAuthentication::SignIn,
    }
    .pair()
}

/// The write after which an attempt fails on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The node and its first revision.
    Node,
    /// The assignment and its first revision.
    Assignment,
    Cards,
    Event,
}

/// Places a test between the steps of the first attempt. Each barrier is waited twice: once
/// when the attempt reaches the point, once to let it continue.
#[derive(Default)]
struct Hooks {
    fail_after: Option<Step>,
    /// After discovery, before the guards.
    after_discovery: Option<Arc<Barrier>>,
    /// After the event append, before the commit.
    before_commit: Option<Arc<Barrier>>,
}

#[derive(Debug, thiserror::Error)]
enum CreationError {
    /// A guard wait timed out. The next attempt rediscovers from the start.
    #[error("guard wait timed out")]
    Retry,
    #[error("injected failure after {0:?}")]
    Injected(Step),
    #[error("guard waits timed out on every attempt")]
    Contention,
    #[error(transparent)]
    Tree(AccountTreeRepoError),
    #[error(transparent)]
    Plan(PlanStorageRepoError),
    /// A storage failure of any participant.
    #[error(transparent)]
    Repo(RepoError),
}

impl From<RepoError> for CreationError {
    fn from(error: RepoError) -> Self {
        if is_guard_timeout(&error) {
            CreationError::Retry
        } else {
            CreationError::Repo(error)
        }
    }
}

impl From<AccountTreeRepoError> for CreationError {
    fn from(error: AccountTreeRepoError) -> Self {
        CreationError::Tree(error)
    }
}

impl From<PlanStorageRepoError> for CreationError {
    fn from(error: PlanStorageRepoError) -> Self {
        CreationError::Plan(error)
    }
}

#[derive(Debug)]
struct Created {
    node: AccountNodeRecord,
    assignment: PlanAssignmentRecord,
}

fn signalled<T>(deps: &Deps, signal: RequiresNotificationSignal<T>) -> T {
    RequiresNotificationSignalExt::signal_new_events_available(
        signal,
        deps.test_registry_change_notifier().as_ref(),
    )
}

fn fail_at(fail_after: Option<Step>, step: Step) -> Result<(), CreationError> {
    if fail_after == Some(step) {
        Err(CreationError::Injected(step))
    } else {
        Ok(())
    }
}

async fn pass(barrier: Option<Arc<Barrier>>) {
    if let Some(barrier) = barrier {
        barrier.wait().await;
        barrier.wait().await;
    }
}

struct Backend<P>(PhantomData<P>);

#[trait_gen(PostgresPool -> PostgresPool, SqlitePool)]
impl Backend<PostgresPool> {
    async fn ensure_fixture_cards(pool: &PostgresPool) {
        let _lock = FIXTURE_CARDS_TABLE_LOCK.lock().await;
        pool.with_rw("account_creation_test", "fixture_cards_table")
            .execute(sqlx::query(FIXTURE_CARDS_TABLE))
            .await
            .unwrap();
    }

    /// One attempt in one transaction. The guards cover the parent and its ancestors (shared),
    /// the new account (exclusive) and the selected plan (shared). The tree and assignment
    /// writes reread the parent and the definition under these guards. The event append comes
    /// last, so the registry journal guard is the last one taken.
    async fn transaction(
        pool: &PostgresPool,
        request: NewChild,
        guards: GuardSet,
        timeout: Duration,
        fail_after: Option<Step>,
        before_commit: Option<Arc<Barrier>>,
    ) -> Result<Created, CreationError> {
        pool.with_tx_err("account_creation_test", "create_child_account", |tx| {
            async move {
                DbGuards::<PostgresPool>::acquire_in_tx(tx, &guards, timeout).await?;
                let node = DbAccountTreeRepo::<PostgresPool>::create_child_in_tx::<CreationError>(
                    tx,
                    request.account_id,
                    request.parent_id,
                    &request.segment,
                    None,
                    request.actor,
                )
                .await?;
                fail_at(fail_after, Step::Node)?;
                let assignment = DbPlanStorageRepo::<PostgresPool>::assign_in_tx::<CreationError>(
                    tx,
                    request.account_id,
                    request.parent_id,
                    None,
                    request.plan_id,
                    request.actor,
                )
                .await?;
                fail_at(fail_after, Step::Assignment)?;
                for effect in FIXTURE_EFFECTS {
                    tx.execute(
                        sqlx::query(
                            "INSERT INTO account_creation_fixture_cards \
                             (card_id, wallet_account_id, holder_issuer, holder_subject, effect) \
                             VALUES ($1, $2, $3, $4, $5)",
                        )
                        .bind(Uuid::new_v4())
                        .bind(request.account_id)
                        .bind(request.pair.issuer.clone())
                        .bind(request.pair.subject.clone())
                        .bind(effect),
                    )
                    .await?;
                }
                fail_at(fail_after, Step::Cards)?;
                DbRegistryChangeRepo::<PostgresPool>::create_change_event_in_tx(
                    tx,
                    &NewRegistryChangeEvent::account_changed(request.account_id),
                )
                .await?;
                fail_at(fail_after, Step::Event)?;
                pass(before_commit).await;
                Ok(Created { node, assignment })
            }
            .boxed()
        })
        .await
    }

    async fn fixture_cards_of(
        pool: &PostgresPool,
        wallet_account_id: Uuid,
    ) -> Vec<(String, String, String)> {
        pool.with_ro("account_creation_test", "fixture_cards_of")
            .fetch_all_as(
                sqlx::query_as(
                    "SELECT holder_issuer, holder_subject, effect \
                     FROM account_creation_fixture_cards WHERE wallet_account_id = $1 ORDER BY effect",
                )
                .bind(wallet_account_id),
            )
            .await
            .unwrap()
    }

    async fn fixture_cards_held_by(
        pool: &PostgresPool,
        holder_subject: String,
    ) -> Vec<(String, String, String)> {
        pool.with_ro("account_creation_test", "fixture_cards_held_by")
            .fetch_all_as(
                sqlx::query_as(
                    "SELECT holder_issuer, holder_subject, effect \
                     FROM account_creation_fixture_cards WHERE holder_subject = $1 ORDER BY effect",
                )
                .bind(holder_subject),
            )
            .await
            .unwrap()
    }
}

async fn ensure_fixture_cards(deps: &Deps) {
    match &deps.test_db {
        TestDb::Postgres(pool) => Backend::<PostgresPool>::ensure_fixture_cards(pool).await,
        TestDb::Sqlite(pool) => Backend::<SqlitePool>::ensure_fixture_cards(pool).await,
    }
}

async fn attempt(
    deps: &Deps,
    request: &NewChild,
    hooks: &mut Hooks,
    timeout: Duration,
) -> Result<Created, CreationError> {
    let ancestry: Vec<Uuid> = deps
        .account_tree_repo
        .get_ancestry(request.parent_id)
        .await?
        .into_iter()
        .map(|node| node.account_id)
        .collect();
    if ancestry.is_empty() {
        return Err(AccountTreeRepoError::ParentNotFound.into());
    }
    pass(hooks.after_discovery.take()).await;
    let mut guards = GuardSet::new();
    guards
        .add_all(GuardClass::Account, ancestry, GuardMode::Shared)
        .add(
            GuardClass::Account,
            request.account_id,
            GuardMode::Exclusive,
        )
        .add(GuardClass::Plan, request.plan_id, GuardMode::Shared);
    let fail_after = hooks.fail_after;
    let before_commit = hooks.before_commit.take();
    match &deps.test_db {
        TestDb::Postgres(pool) => {
            Backend::<PostgresPool>::transaction(
                pool,
                request.clone(),
                guards,
                timeout,
                fail_after,
                before_commit,
            )
            .await
        }
        TestDb::Sqlite(pool) => {
            Backend::<SqlitePool>::transaction(
                pool,
                request.clone(),
                guards,
                timeout,
                fail_after,
                before_commit,
            )
            .await
        }
    }
}

/// Creates the child with bounded retries after guard timeouts. Every attempt keeps the
/// request's account ID, pair and expected first assignment. The caller signals after commit.
async fn create_with(
    deps: &Deps,
    request: &NewChild,
    mut hooks: Hooks,
    timeout: Duration,
    max_attempts: usize,
) -> Result<RequiresNotificationSignal<Created>, CreationError> {
    ensure_fixture_cards(deps).await;
    for attempt_number in 1..=max_attempts {
        match attempt(deps, request, &mut hooks, timeout).await {
            Err(CreationError::Retry) => {
                tokio::time::sleep(Duration::from_millis(5 * attempt_number as u64)).await
            }
            result => return result.map(|created| created.requires_notification_signal()),
        }
    }
    Err(CreationError::Contention)
}

async fn create(
    deps: &Deps,
    request: &NewChild,
    hooks: Hooks,
) -> Result<RequiresNotificationSignal<Created>, CreationError> {
    create_with(deps, request, hooks, GUARD_TIMEOUT, MAX_ATTEMPTS).await
}

async fn fixture_cards_of(deps: &Deps, account_id: Uuid) -> Vec<(String, String, String)> {
    match &deps.test_db {
        TestDb::Postgres(pool) => Backend::<PostgresPool>::fixture_cards_of(pool, account_id).await,
        TestDb::Sqlite(pool) => Backend::<SqlitePool>::fixture_cards_of(pool, account_id).await,
    }
}

async fn fixture_cards_held_by(deps: &Deps, pair: &Pair) -> Vec<(String, String, String)> {
    let subject = pair.subject.clone();
    match &deps.test_db {
        TestDb::Postgres(pool) => {
            Backend::<PostgresPool>::fixture_cards_held_by(pool, subject).await
        }
        TestDb::Sqlite(pool) => Backend::<SqlitePool>::fixture_cards_held_by(pool, subject).await,
    }
}

/// What a separate reader sees of one creation. Reads only; a creation must have ensured the
/// fixture Cards table first, because a write here would wait for a paused creation.
#[derive(Debug, Default, PartialEq)]
struct Persisted {
    node: bool,
    listed: bool,
    node_revisions: usize,
    assignment: bool,
    assignment_revisions: usize,
    cards: usize,
    events: usize,
}

async fn persisted(
    deps: &Deps,
    request: &NewChild,
    since: golem_registry_service::repo::registry_change::ChangeEventId,
) -> Persisted {
    let id = request.account_id;
    Persisted {
        node: deps.account_tree_repo.get(id).await.unwrap().is_some(),
        listed: deps
            .account_tree_repo
            .list_children(request.parent_id)
            .await
            .unwrap()
            .iter()
            .any(|node| node.account_id == id),
        node_revisions: deps.account_tree_repo.get_history(id).await.unwrap().len(),
        assignment: deps
            .plan_storage_repo
            .get_assignment(id)
            .await
            .unwrap()
            .is_some(),
        assignment_revisions: deps
            .plan_storage_repo
            .get_assignment_history(id)
            .await
            .unwrap()
            .len(),
        cards: fixture_cards_of(deps, id).await.len(),
        events: account_events_since(deps, since)
            .await
            .into_iter()
            .filter(|account_id| *account_id == id)
            .count(),
    }
}

fn complete() -> Persisted {
    Persisted {
        node: true,
        listed: true,
        node_revisions: 1,
        assignment: true,
        assignment_revisions: 1,
        cards: 2,
        events: 1,
    }
}

/// A depth-one account with a root-owned plan of [12, 100], and the plan it offers its
/// children, [3, 20], edited once so that its current revision is 1.
struct Parent {
    node: AccountNodeRecord,
    /// The root-owned plan the parent is assigned.
    parent_plan: Uuid,
    plan_id: Uuid,
}

async fn parent(deps: &Deps) -> Parent {
    let root = root(deps).await;
    let node = child(deps, root.account_id, &unique_segment(20)).await;
    let parent_plan = define(deps, root.account_id, range(Some(12), 100)).await;
    deps.plan_storage_repo
        .assign(node.account_id, None, parent_plan, Uuid::new_v4())
        .await
        .unwrap();
    let plan_id = define(deps, node.account_id, range(Some(3), 30)).await;
    let_assert!(Ok(1) = edit(deps.plan_storage_repo.as_ref(), plan_id, range(Some(3), 20)).await);
    Parent {
        node,
        parent_plan,
        plan_id,
    }
}

/// How long a competitor must stay blocked.
const BLOCKED_FOR: Duration = Duration::from_millis(500);

/// Polls `competitor` until `BLOCKED_FOR` passes and checks that it has not finished: it waits
/// for a guard or the write lock that the paused creation holds.
async fn assert_blocked<F: Future + Unpin>(competitor: &mut F) {
    assert!(
        tokio::time::timeout(BLOCKED_FOR, competitor).await.is_err(),
        "the competitor finished while the creation held its guards"
    );
}

fn segment(value: &str) -> AccountSegment {
    AccountSegment::from_str(value).unwrap()
}

pub async fn test_account_creation_commits_every_participant(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let created = signalled(
        deps,
        create(deps, &request, Hooks::default()).await.unwrap(),
    );

    assert!(persisted(deps, &request, since).await == complete());
    let node = deps
        .account_tree_repo
        .get(request.account_id)
        .await
        .unwrap()
        .unwrap();
    assert!(node == created.node);
    assert!(node.parent_id == Some(parent.node.account_id));
    assert!(node.segment == "c");
    assert!(node.depth == 2);
    assert!(node.current_revision == 0);
    assert!(node.modified_by == request.actor);

    let assignment = deps
        .plan_storage_repo
        .get_assignment(request.account_id)
        .await
        .unwrap()
        .unwrap();
    assert!(assignment == created.assignment);
    assert!(assignment.parent_id == parent.node.account_id);
    assert!(assignment.plan_id == parent.plan_id);
    assert!(assignment.revision == 0);
    let history = deps
        .plan_storage_repo
        .get_assignment_history(request.account_id)
        .await
        .unwrap();
    let_assert!([revision] = history.as_slice());
    assert!(revision.plan_id == parent.plan_id);
    assert!(revision.plan_revision == 1);
    assert!(revision.created_by == request.actor);

    // The minimum comes from the parent's plan and the maximum from the child's own plan.
    let_assert!(
        Ok(EffectiveBounds::Range(effective)) = table_bounds(deps, request.account_id).await
    );
    assert!(effective.min.value == 12);
    assert!(effective.min.source == EndpointSource::Level(1));
    assert!(effective.max.value == DeclaredMax::Finite(20));
    assert!(effective.max.source == EndpointSource::Level(2));

    // Both fixture effects belong to the new account's wallet; the creator holds none.
    let subject = format!("account:{}", request.account_id.hyphenated());
    let cards = fixture_cards_of(deps, request.account_id).await;
    assert!(
        cards
            == FIXTURE_EFFECTS
                .map(|effect| (
                    request.pair.issuer.clone(),
                    subject.clone(),
                    effect.to_string()
                ))
                .to_vec()
    );
    let creator = account_pair(deps, request.actor).await;
    assert!(fixture_cards_held_by(deps, &creator).await.is_empty());
}

/// An attempt that fails after any write leaves nothing, and the same request then succeeds
/// with the same account ID and sibling name.
pub async fn test_account_creation_failures_leave_nothing(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    for step in [Step::Node, Step::Assignment, Step::Cards, Step::Event] {
        let hooks = Hooks {
            fail_after: Some(step),
            ..Hooks::default()
        };
        let_assert!(Err(CreationError::Injected(failed)) = create(deps, &request, hooks).await);
        assert!(failed == step);
        assert!(persisted(deps, &request, since).await == Persisted::default());
    }
    create(deps, &request, Hooks::default())
        .await
        .unwrap()
        .signal_new_events_available_for_test(deps);
    assert!(persisted(deps, &request, since).await == complete());
}

/// Two creations that claim the same sibling name: one commits completely, the other leaves
/// nothing.
pub async fn test_account_creation_sibling_race(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let name = unique_segment(10);
    let first = NewChild::new(deps, parent.node.account_id, name.clone(), parent.plan_id).await;
    let second = NewChild::new(deps, parent.node.account_id, name, parent.plan_id).await;
    let barrier = Arc::new(Barrier::new(3));
    let hooks = || Hooks {
        after_discovery: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (first_result, second_result, ()) = tokio::join!(
        create(deps, &first, hooks()),
        create(deps, &second, hooks()),
        async {
            barrier.wait().await;
            barrier.wait().await;
        }
    );
    let (winner, loser, error) = match (first_result, second_result) {
        (Ok(signal), Err(error)) => {
            signal.signal_new_events_available_for_test(deps);
            (&first, &second, error)
        }
        (Err(error), Ok(signal)) => {
            signal.signal_new_events_available_for_test(deps);
            (&second, &first, error)
        }
        (first, second) => panic!(
            "expected one winner, got {:?} and {:?}",
            first.err(),
            second.err()
        ),
    };
    let_assert!(CreationError::Tree(AccountTreeRepoError::SiblingConflict) = error);
    assert!(persisted(deps, winner, since).await == complete());
    assert!(persisted(deps, loser, since).await == Persisted::default());
}

/// A plan that retires or becomes incompatible after creation's discovery is rechecked under
/// the guards and refused; a plan edit that waits for the guards of a paused creation sees the
/// new member and is refused instead.
pub async fn test_account_creation_revalidates_plan_after_wait(deps: &Deps) {
    let since = latest_event_id(deps).await;

    let retired = parent(deps).await;
    let request = NewChild::new(deps, retired.node.account_id, segment("c"), retired.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        after_discovery: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, ()) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        deps.plan_storage_repo
            .retire_definition(retired.plan_id, 1, Uuid::new_v4())
            .await
            .unwrap();
        barrier.wait().await;
    });
    let_assert!(Err(CreationError::Plan(PlanStorageRepoError::NotSelectable)) = result);
    assert!(persisted(deps, &request, since).await == Persisted::default());

    // [3, 8] is disjoint from the parent's [12, 100].
    let edited = parent(deps).await;
    let request = NewChild::new(deps, edited.node.account_id, segment("c"), edited.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        after_discovery: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, ()) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        let_assert!(
            Ok(2) = edit(
                deps.plan_storage_repo.as_ref(),
                edited.plan_id,
                range(Some(3), 8)
            )
            .await
        );
        barrier.wait().await;
    });
    let_assert!(
        Err(CreationError::Plan(
            PlanStorageRepoError::Incompatible { .. }
        )) = result
    );
    assert!(persisted(deps, &request, since).await == Persisted::default());

    let guarded = parent(deps).await;
    let request = NewChild::new(deps, guarded.node.account_id, segment("c"), guarded.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, edit_result) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        let mut edit_task = Box::pin(edit(
            deps.plan_storage_repo.as_ref(),
            guarded.plan_id,
            range(Some(3), 8),
        ));
        assert_blocked(&mut edit_task).await;
        let (edit_result, ()) = tokio::join!(edit_task, barrier.wait().map(|_| ()));
        edit_result
    });
    result.unwrap().signal_new_events_available_for_test(deps);
    let_assert!(Err(PlanStorageRepoError::Incompatible { account_id, .. }) = edit_result);
    assert!(account_id == request.account_id);
    assert!(persisted(deps, &request, since).await == complete());
    let definition = deps
        .plan_storage_repo
        .get_definition(guarded.plan_id)
        .await
        .unwrap()
        .unwrap();
    assert!(definition.record.current_revision == 1);
}

/// A parent purged after creation's discovery leaves no orphan; a purge that waits for the
/// guards of a paused creation finds the new child and is refused.
pub async fn test_account_creation_parent_disappears(deps: &Deps) {
    let since = latest_event_id(deps).await;

    let gone = parent(deps).await;
    let grace = update(deps, &gone.node, None, AccountLifecycleState::DeletionGrace)
        .await
        .unwrap();
    let request = NewChild::new(deps, gone.node.account_id, segment("c"), gone.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        after_discovery: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, ()) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        deps.account_tree_repo
            .purge(
                grace.account_id,
                grace.current_revision as u64,
                Uuid::new_v4(),
            )
            .await
            .unwrap()
            .signal_new_events_available_for_test(deps);
        barrier.wait().await;
    });
    let_assert!(Err(CreationError::Tree(AccountTreeRepoError::ParentNotFound)) = result);
    assert!(persisted(deps, &request, since).await == Persisted::default());

    let kept = parent(deps).await;
    let grace = update(deps, &kept.node, None, AccountLifecycleState::DeletionGrace)
        .await
        .unwrap();
    let request = NewChild::new(deps, kept.node.account_id, segment("c"), kept.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, purge_result) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        let mut purge = deps.account_tree_repo.purge(
            grace.account_id,
            grace.current_revision as u64,
            Uuid::new_v4(),
        );
        assert_blocked(&mut purge).await;
        let (purge_result, ()) = tokio::join!(purge, barrier.wait().map(|_| ()));
        purge_result
    });
    result.unwrap().signal_new_events_available_for_test(deps);
    let_assert!(Err(AccountTreeRepoError::HasChildren) = purge_result);
    assert!(persisted(deps, &request, since).await == complete());
}

/// An edit that narrows the parent's own plan while a creation is paused cannot commit a range
/// that the new child's plan contradicts. Creation checks the child against the old parent
/// bounds, and the edit checks a subtree that does not hold the uncommitted child. On
/// PostgreSQL only the creation's shared ancestor guards, which the edit's exclusive guard on
/// its member waits for, order the two. No row lock or foreign key orders them: the edit writes
/// only the parent plan's rows, and the creation's keys reference the parent node and the
/// child's plan. SQLite orders them by its single writer.
pub async fn test_account_creation_orders_ancestor_plan_edits(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, edit_result) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        // [25, 100] leaves the parent valid and is disjoint from the child's [3, 20].
        let mut edit_task = Box::pin(edit(
            deps.plan_storage_repo.as_ref(),
            parent.parent_plan,
            range(Some(25), 100),
        ));
        // The edit may finish here only if nothing orders it after the creation.
        let early = tokio::time::timeout(BLOCKED_FOR, &mut edit_task).await;
        barrier.wait().await;
        match early {
            Ok(edit_result) => edit_result,
            Err(_) => edit_task.await,
        }
    });
    result.unwrap().signal_new_events_available_for_test(deps);
    let_assert!(
        Ok(EffectiveBounds::Range(effective)) = table_bounds(deps, request.account_id).await
    );
    assert!(effective.min.value == 12);
    assert!(effective.max.value == DeclaredMax::Finite(20));
    let_assert!(Err(PlanStorageRepoError::Incompatible { account_id, .. }) = edit_result);
    assert!(account_id == request.account_id);
    assert!(persisted(deps, &request, since).await == complete());
    let definition = deps
        .plan_storage_repo
        .get_definition(parent.parent_plan)
        .await
        .unwrap()
        .unwrap();
    assert!(definition.record.current_revision == 0);
}

/// A separate reader sees nothing of a creation before it commits, and everything after.
pub async fn test_account_creation_is_invisible_until_commit(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, ()) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        assert!(persisted(deps, &request, since).await == Persisted::default());
        barrier.wait().await;
    });
    result.unwrap().signal_new_events_available_for_test(deps);
    assert!(persisted(deps, &request, since).await == complete());
}

/// An attempt cancelled after its writes leaves nothing; the same request then commits once.
pub async fn test_account_creation_cancellation_rolls_back(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    {
        let cancelled = create(deps, &request, hooks);
        tokio::pin!(cancelled);
        tokio::select! {
            _ = &mut cancelled => panic!("creation finished before the barrier"),
            _ = barrier.wait() => {}
        }
    }
    assert!(persisted(deps, &request, since).await == Persisted::default());
    create(deps, &request, Hooks::default())
        .await
        .unwrap()
        .signal_new_events_available_for_test(deps);
    assert!(persisted(deps, &request, since).await == complete());
}

/// A guard wait that expires on every attempt fails with `Contention` and leaves nothing;
/// the same request commits once the guard is released. PostgreSQL only: SQLite takes no
/// guards.
pub async fn test_account_creation_guard_timeout_retries(deps: &Deps) {
    let parent = parent(deps).await;
    let since = latest_event_id(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let mut held = GuardSet::new();
    held.add(
        GuardClass::Account,
        parent.node.account_id,
        GuardMode::Exclusive,
    );
    let tx = hold_guard(deps, held).await;
    let_assert!(
        Err(CreationError::Contention) = create_with(
            deps,
            &request,
            Hooks::default(),
            Duration::from_millis(200),
            2
        )
        .await
    );
    tx.rollback().await.unwrap();
    assert!(persisted(deps, &request, since).await == Persisted::default());
    let created = signalled(
        deps,
        create(deps, &request, Hooks::default()).await.unwrap(),
    );
    assert!(created.node.account_id == request.account_id);
    assert!(persisted(deps, &request, since).await == complete());
}

async fn next_account_event(
    receiver: &mut broadcast::Receiver<RegistryChangeEvent>,
    ids: &[Uuid],
) -> Uuid {
    loop {
        if let RegistryChangeEvent::AccountChanged { account_id, .. } =
            receiver.recv().await.unwrap()
            && ids.contains(&account_id)
        {
            return account_id;
        }
    }
}

/// The SQLite notifier delivers only committed events, after the commit signal: a rolled-back
/// attempt sends no signal and its event never reaches a subscriber.
pub async fn test_account_creation_sqlite_signals_after_commit(deps: &Deps) {
    let_assert!(TestDb::Sqlite(pool) = &deps.test_db);
    let repo: Arc<dyn RegistryChangeRepo> = Arc::new(DbRegistryChangeRepo::new(pool.clone()));
    let notifier = SqliteRegistryChangeNotifier::new(1024, repo);
    let mut join_set = tokio::task::JoinSet::new();
    notifier.start_background_tasks(&mut join_set);
    let mut receiver = notifier.subscribe();

    let parent = parent(deps).await;
    let failed = NewChild::new(deps, parent.node.account_id, segment("f"), parent.plan_id).await;
    let hooks = Hooks {
        fail_after: Some(Step::Event),
        ..Hooks::default()
    };
    let_assert!(Err(CreationError::Injected(Step::Event)) = create(deps, &failed, hooks).await);
    let committed = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let signal = create(deps, &committed, Hooks::default()).await.unwrap();
    RequiresNotificationSignalExt::signal_new_events_available(signal, &notifier);
    let delivered = tokio::time::timeout(
        Duration::from_secs(5),
        next_account_event(&mut receiver, &[failed.account_id, committed.account_id]),
    )
    .await
    .unwrap();
    assert!(delivered == committed.account_id);
    join_set.abort_all();
}

/// PostgreSQL's transactional notification reaches a listener only at commit.
pub async fn test_account_creation_postgres_notifies_at_commit(
    deps: &Deps,
    config: &golem_common::config::DbPostgresConfig,
) {
    let_assert!(TestDb::Postgres(pool) = &deps.test_db);
    let repo: Arc<dyn RegistryChangeRepo> = Arc::new(DbRegistryChangeRepo::new(pool.clone()));
    let notifier = PostgresRegistryChangeNotifier::new(1024, repo, config);
    let mut join_set = tokio::task::JoinSet::new();
    notifier.start_background_tasks(&mut join_set);
    let mut receiver = notifier.subscribe();

    // The listener is connected once a probe event committed after subscribing arrives.
    let probe = Uuid::new_v4();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            deps.record_registry_change_event(NewRegistryChangeEvent::account_changed(probe))
                .await;
            if tokio::time::timeout(
                Duration::from_millis(500),
                next_account_event(&mut receiver, &[probe]),
            )
            .await
            .is_ok()
            {
                break;
            }
        }
    })
    .await
    .unwrap();

    let parent = parent(deps).await;
    let request = NewChild::new(deps, parent.node.account_id, segment("c"), parent.plan_id).await;
    let barrier = Arc::new(Barrier::new(2));
    let hooks = Hooks {
        before_commit: Some(barrier.clone()),
        ..Hooks::default()
    };
    let (result, ()) = tokio::join!(create(deps, &request, hooks), async {
        barrier.wait().await;
        // The event row is written but not committed; no notification may arrive yet.
        assert!(
            tokio::time::timeout(
                Duration::from_millis(500),
                next_account_event(&mut receiver, &[request.account_id]),
            )
            .await
            .is_err()
        );
        barrier.wait().await;
    });
    signalled(deps, result.unwrap());
    let delivered = tokio::time::timeout(
        Duration::from_secs(5),
        next_account_event(&mut receiver, &[request.account_id]),
    )
    .await
    .unwrap();
    assert!(delivered == request.account_id);
    join_set.abort_all();
}
