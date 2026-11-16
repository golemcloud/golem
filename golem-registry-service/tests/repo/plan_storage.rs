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

use crate::repo::account_tree::{SignalForTest, child, root, unique_segment, update};
use crate::repo::{Deps, TestDb};
use assert2::{assert, let_assert};
use futures::FutureExt;
use golem_common::model::account_path::AccountSegment;
use golem_common::model::plan_dimension::{
    DeclaredBounds, DeclaredMax, DimensionBoundsError, DimensionDeclaration, EffectiveBounds,
    EndpointSource, InstallationAvailability, PlanDimension,
};
use golem_registry_service::repo::account_guard::{
    DbGuards, GuardClass, GuardMode, GuardSet, guard_key,
};
use golem_registry_service::repo::model::account_tree::{AccountLifecycleState, AccountNodeRecord};
use golem_registry_service::repo::model::plan_storage::{PlanPolicy, PlanStorageRepoError};
use golem_registry_service::repo::plan_storage::{DbPlanStorageRepo, PlanStorageRepo};
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::{LabelledPoolApi, LabelledPoolTransaction, Pool, PoolApi};
use std::str::FromStr;
use std::time::{Duration, Instant};
use tracing::info;
use uuid::Uuid;

const DIMENSION: PlanDimension = PlanDimension::MaxTableElementsPerAgent;

fn range(min: Option<u64>, max: u64) -> PlanPolicy {
    PlanPolicy::from([(
        DIMENSION,
        DimensionDeclaration {
            bounds: DeclaredBounds {
                min,
                max: Some(DeclaredMax::Finite(max)),
            },
            operating: None,
        },
    )])
}

fn name() -> String {
    format!("plan-{}", Uuid::new_v4().simple())
}

fn actor() -> Uuid {
    Uuid::new_v4()
}

async fn define(deps: &Deps, owner: Uuid, policy: PlanPolicy) -> Uuid {
    let plan_id = Uuid::new_v4();
    deps.plan_storage_repo
        .create_definition(owner, plan_id, name(), policy, actor())
        .await
        .unwrap();
    plan_id
}

async fn assign(deps: &Deps, account: Uuid, plan_id: Uuid) {
    let current = deps
        .plan_storage_repo
        .get_assignment(account)
        .await
        .unwrap()
        .map(|assignment| assignment.revision as u64);
    deps.plan_storage_repo
        .assign(account, current, plan_id, actor())
        .await
        .unwrap();
}

async fn edit(
    repo: &dyn PlanStorageRepo,
    plan_id: Uuid,
    policy: PlanPolicy,
) -> Result<u64, PlanStorageRepoError> {
    let current = repo.get_definition(plan_id).await.unwrap().unwrap();
    repo.edit_definition(
        plan_id,
        current.record.current_revision as u64,
        current.record.name,
        policy,
        actor(),
    )
    .await
    .map(|definition| definition.record.current_revision as u64)
}

async fn table_bounds(deps: &Deps, account: Uuid) -> Result<EffectiveBounds, DimensionBoundsError> {
    deps.plan_storage_repo
        .effective_bounds(account, InstallationAvailability::Available)
        .await
        .unwrap()
        .unwrap()
        .remove(&DIMENSION)
        .unwrap()
}

/// A depth-one account `a` with a root-owned plan and its child `b` with an `a`-owned plan.
struct Branch {
    a: AccountNodeRecord,
    b: AccountNodeRecord,
    parent_plan: Uuid,
    child_plan: Uuid,
}

async fn branch(deps: &Deps, parent: PlanPolicy, child_policy: PlanPolicy) -> Branch {
    let root = root(deps).await;
    let a = child(deps, root.account_id, &unique_segment(20)).await;
    let b = child(deps, a.account_id, &AccountSegment::from_str("b").unwrap()).await;
    let parent_plan = define(deps, root.account_id, parent).await;
    let child_plan = define(deps, a.account_id, child_policy).await;
    assign(deps, a.account_id, parent_plan).await;
    assign(deps, b.account_id, child_plan).await;
    Branch {
        a,
        b,
        parent_plan,
        child_plan,
    }
}

pub async fn test_plan_definitions_are_versioned_and_owner_scoped(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let root = root(deps).await;
    let a = child(deps, root.account_id, &unique_segment(20)).await;
    let plan_id = Uuid::new_v4();
    let plan_name = name();
    let created = repo
        .create_definition(
            root.account_id,
            plan_id,
            plan_name.clone(),
            range(Some(1), 100),
            actor(),
        )
        .await
        .unwrap();
    assert!(created.record.current_revision == 0);

    let_assert!(
        Err(PlanStorageRepoError::NameConflict) = repo
            .create_definition(
                root.account_id,
                Uuid::new_v4(),
                plan_name.clone(),
                range(None, 5),
                actor()
            )
            .await
    );
    // Names are local to their owner.
    repo.create_definition(
        a.account_id,
        Uuid::new_v4(),
        plan_name.clone(),
        range(None, 5),
        actor(),
    )
    .await
    .unwrap();

    let edited = repo
        .edit_definition(plan_id, 0, plan_name.clone(), range(Some(2), 50), actor())
        .await
        .unwrap();
    assert!(edited.record.current_revision == 1);
    let_assert!(
        Err(PlanStorageRepoError::ConcurrentModification) = repo
            .edit_definition(plan_id, 0, plan_name.clone(), range(Some(3), 40), actor())
            .await
    );
    assert!(repo.get_definition_revision(plan_id, 0).await.unwrap() == Some(range(Some(1), 100)));
    assert!(repo.get_definition_revision(plan_id, 1).await.unwrap() == Some(range(Some(2), 50)));
    assert!(repo.get_definition(plan_id).await.unwrap().unwrap().policy == range(Some(2), 50));

    // A single declaration must itself be valid in the dimension's domain.
    let_assert!(
        Err(PlanStorageRepoError::InvalidDeclaration { .. }) = repo
            .edit_definition(plan_id, 1, plan_name.clone(), range(Some(10), 5), actor())
            .await
    );
    let zero_rate = PlanPolicy::from([(
        PlanDimension::OplogWritesPerSecond,
        DimensionDeclaration {
            bounds: DeclaredBounds {
                min: None,
                max: Some(DeclaredMax::Finite(0)),
            },
            operating: None,
        },
    )]);
    let_assert!(
        Err(PlanStorageRepoError::InvalidDeclaration {
            dimension: PlanDimension::OplogWritesPerSecond,
            error: DimensionBoundsError::OutsideDomain { .. },
        }) = repo
            .create_definition(root.account_id, Uuid::new_v4(), name(), zero_rate, actor())
            .await
    );
}

pub async fn test_plan_assignment_follows_owner_and_latest_revision(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let root = root(deps).await;
    let a = child(deps, root.account_id, &unique_segment(20)).await;
    let b = child(deps, a.account_id, &AccountSegment::from_str("b").unwrap()).await;
    let parent_plan = define(deps, root.account_id, range(Some(1), 100)).await;
    let own_plan = define(deps, a.account_id, range(Some(1), 20)).await;

    let_assert!(
        Err(PlanStorageRepoError::ForeignCatalog) =
            repo.assign(a.account_id, None, own_plan, actor()).await
    );
    let_assert!(
        Err(PlanStorageRepoError::ForeignCatalog) =
            repo.assign(b.account_id, None, parent_plan, actor()).await
    );
    let_assert!(
        Err(PlanStorageRepoError::RootAssignment) = repo
            .assign(root.account_id, None, parent_plan, actor())
            .await
    );

    // Assignment uses the definition revision current at commit.
    assert!(
        edit(repo.as_ref(), parent_plan, range(Some(10), 30))
            .await
            .unwrap()
            == 1
    );
    let assigned = repo
        .assign(a.account_id, None, parent_plan, actor())
        .await
        .unwrap();
    assert!(assigned.revision == 0);
    let_assert!(
        Err(PlanStorageRepoError::ConcurrentModification) =
            repo.assign(a.account_id, None, parent_plan, actor()).await
    );
    repo.assign(b.account_id, None, own_plan, actor())
        .await
        .unwrap();

    // The assignment follows a later edit without a new assignment revision.
    assert!(
        edit(repo.as_ref(), parent_plan, range(Some(12), 30))
            .await
            .unwrap()
            == 2
    );
    let history = repo.get_assignment_history(a.account_id).await.unwrap();
    assert!(history.len() == 1);
    assert!(history[0].plan_revision == 1);

    // Largest minimum from the parent level, smallest maximum from the child level.
    let_assert!(Ok(EffectiveBounds::Range(effective)) = table_bounds(deps, b.account_id).await);
    assert!(effective.min.value == 12);
    assert!(effective.min.source == EndpointSource::Level(1));
    assert!(effective.max.value == DeclaredMax::Finite(20));
    assert!(effective.max.source == EndpointSource::Level(2));
}

pub async fn test_plan_writes_refuse_impossible_ranges(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let setup = branch(deps, range(Some(1), 100), range(Some(1), 20)).await;

    let_assert!(
        Err(PlanStorageRepoError::Incompatible {
            account_id,
            dimension: DIMENSION,
            ..
        }) = edit(repo.as_ref(), setup.parent_plan, range(Some(25), 100)).await
    );
    assert!(account_id == setup.b.account_id);
    // Touching ranges keep one valid value.
    edit(repo.as_ref(), setup.parent_plan, range(Some(20), 100))
        .await
        .unwrap();

    // An unused incompatible definition does not block its owner's ancestors, but cannot be
    // assigned.
    let narrow = define(deps, setup.a.account_id, range(None, 7)).await;
    edit(repo.as_ref(), setup.parent_plan, range(Some(19), 100))
        .await
        .unwrap();
    let_assert!(
        Err(PlanStorageRepoError::Incompatible { .. }) = repo
            .assign(setup.b.account_id, Some(0), narrow, actor())
            .await
    );
    assert!(
        repo.get_assignment(setup.b.account_id)
            .await
            .unwrap()
            .unwrap()
            .plan_id
            == setup.child_plan
    );

    // A member in deletion grace still constrains a shared edit.
    update(deps, &setup.b, None, AccountLifecycleState::DeletionGrace)
        .await
        .unwrap();
    let_assert!(
        Err(PlanStorageRepoError::Incompatible { .. }) =
            edit(repo.as_ref(), setup.parent_plan, range(Some(25), 100)).await
    );

    // An assigned definition cannot be retired; a retired one cannot be assigned and frees its
    // name.
    let current = repo
        .get_definition(setup.child_plan)
        .await
        .unwrap()
        .unwrap();
    let_assert!(
        Err(PlanStorageRepoError::Assigned) = repo
            .retire_definition(
                setup.child_plan,
                current.record.current_revision as u64,
                actor()
            )
            .await
    );
    let narrow_def = repo.get_definition(narrow).await.unwrap().unwrap();
    repo.retire_definition(narrow, 0, actor()).await.unwrap();
    let_assert!(
        Err(PlanStorageRepoError::NotSelectable) = repo
            .assign(setup.b.account_id, Some(0), narrow, actor())
            .await
    );
    repo.create_definition(
        setup.a.account_id,
        Uuid::new_v4(),
        narrow_def.record.name,
        range(None, 50),
        actor(),
    )
    .await
    .unwrap();
}

/// Races an ancestor minimum increase from 1 to 10 against a descendant maximum decrease from
/// 20 to 7. Whichever commits second must refuse.
pub async fn test_plan_opposing_writes_cannot_commit_impossible_range(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    for order in ["parent-first", "child-first"] {
        let setup = branch(deps, range(Some(1), 100), range(Some(1), 20)).await;
        let (first, second) = if order == "parent-first" {
            (
                edit(repo.as_ref(), setup.parent_plan, range(Some(10), 100)).await,
                edit(repo.as_ref(), setup.child_plan, range(Some(1), 7)).await,
            )
        } else {
            let first = edit(repo.as_ref(), setup.child_plan, range(Some(1), 7)).await;
            let second = edit(repo.as_ref(), setup.parent_plan, range(Some(10), 100)).await;
            (first, second)
        };
        assert!(first.is_ok());
        let_assert!(Err(PlanStorageRepoError::Incompatible { .. }) = second);
        let_assert!(Ok(EffectiveBounds::Range(_)) = table_bounds(deps, setup.b.account_id).await);
    }

    let mut committed = [0usize; 2];
    let mut slowest = Duration::ZERO;
    for _ in 0..10 {
        let setup = branch(deps, range(Some(1), 100), range(Some(1), 20)).await;
        let parent_def = repo
            .get_definition(setup.parent_plan)
            .await
            .unwrap()
            .unwrap();
        let child_def = repo
            .get_definition(setup.child_plan)
            .await
            .unwrap()
            .unwrap();
        let start = Instant::now();
        let (parent, child) = tokio::join!(
            repo.edit_definition(
                setup.parent_plan,
                0,
                parent_def.record.name,
                range(Some(10), 100),
                actor()
            ),
            repo.edit_definition(
                setup.child_plan,
                0,
                child_def.record.name,
                range(Some(1), 7),
                actor()
            ),
        );
        slowest = slowest.max(start.elapsed());
        assert!(!(parent.is_ok() && child.is_ok()));
        for (index, outcome) in [&parent, &child].into_iter().enumerate() {
            match outcome {
                Ok(_) => committed[index] += 1,
                Err(PlanStorageRepoError::Incompatible { .. }) => {}
                Err(other) => panic!("unexpected outcome: {other}"),
            }
        }
        let_assert!(Ok(EffectiveBounds::Range(_)) = table_bounds(deps, setup.b.account_id).await);
    }
    info!(
        parent_commits = committed[0],
        child_commits = committed[1],
        slowest_pair_ms = slowest.as_millis() as u64,
        "opposing plan writes"
    );
}

pub async fn test_plan_retire_races_assignment(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    for _ in 0..5 {
        let root = root(deps).await;
        let a = child(deps, root.account_id, &unique_segment(20)).await;
        let plan_id = define(deps, root.account_id, range(None, 10)).await;
        let (retired, assigned) = tokio::join!(
            repo.retire_definition(plan_id, 0, actor()),
            repo.assign(a.account_id, None, plan_id, actor()),
        );
        match (&retired, &assigned) {
            (Ok(()), Err(PlanStorageRepoError::NotSelectable)) => {
                assert!(repo.get_assignment(a.account_id).await.unwrap().is_none());
            }
            (Err(PlanStorageRepoError::Assigned), Ok(_)) => {
                assert!(
                    repo.get_definition(plan_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .record
                        .selectable
                );
            }
            other => panic!("unexpected outcomes: {other:?}"),
        }
    }
}

pub async fn test_plan_seed_keeps_later_edits(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let root = root(deps).await;
    let plan_id = Uuid::new_v4();
    let plan_name = name();
    assert!(
        repo.seed_definition(
            root.account_id,
            plan_id,
            plan_name.clone(),
            range(None, 10),
            actor()
        )
        .await
        .unwrap()
    );
    edit(repo.as_ref(), plan_id, range(None, 99)).await.unwrap();
    // A restart seeds the same definition again.
    assert!(
        !repo
            .seed_definition(
                root.account_id,
                plan_id,
                plan_name,
                range(None, 10),
                actor()
            )
            .await
            .unwrap()
    );
    let current = repo.get_definition(plan_id).await.unwrap().unwrap();
    assert!(current.record.current_revision == 1);
    assert!(current.policy == range(None, 99));
}

pub async fn test_plan_edit_scales_with_members(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let root = root(deps).await;
    let owner = child(deps, root.account_id, &unique_segment(20)).await;
    let plan_id = define(deps, owner.account_id, range(Some(1), 100)).await;
    let mut measurements = Vec::new();
    let mut members = 0;
    for target in [10usize, 50, 200] {
        while members < target {
            let member = child(deps, owner.account_id, &unique_segment(20)).await;
            assign(deps, member.account_id, plan_id).await;
            members += 1;
        }
        let start = Instant::now();
        edit(repo.as_ref(), plan_id, range(Some(2), 100))
            .await
            .unwrap();
        measurements.push((target, start.elapsed().as_millis() as u64));
    }
    assert!(repo.list_members(plan_id).await.unwrap().len() == 200);
    info!(
        ?measurements,
        "shared definition edit by member count (members, ms)"
    );
}

pub async fn test_tree_purge_releases_plan_records(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let setup = branch(deps, range(None, 100), range(None, 20)).await;
    let owned = define(deps, setup.b.account_id, range(None, 5)).await;
    let b = update(deps, &setup.b, None, AccountLifecycleState::DeletionGrace)
        .await
        .unwrap();
    deps.account_tree_repo
        .purge(b.account_id, b.current_revision as u64, actor())
        .await
        .unwrap()
        .signal_new_events_available_for_test(deps);
    assert!(repo.get_assignment(b.account_id).await.unwrap().is_none());
    assert!(
        repo.get_assignment_history(b.account_id)
            .await
            .unwrap()
            .len()
            == 1
    );
    assert!(repo.get_definition(owned).await.unwrap().is_none());
    assert!(repo.get_definition_revision(owned, 0).await.unwrap() == Some(range(None, 5)));
    assert!(
        repo.list_members(setup.child_plan)
            .await
            .unwrap()
            .is_empty()
    );
}

// Opposing PostgreSQL transactions -----------------------------------------------------------

fn postgres(deps: &Deps) -> &PostgresPool {
    match &deps.test_db {
        TestDb::Postgres(pool) => pool,
        TestDb::Sqlite(_) => panic!("PostgreSQL only"),
    }
}

fn short_repo(deps: &Deps, timeout: Duration, attempts: usize) -> DbPlanStorageRepo<PostgresPool> {
    DbPlanStorageRepo::with_limits(postgres(deps).clone(), timeout, attempts)
}

async fn hold_guard(
    deps: &Deps,
    guards: GuardSet,
) -> <<PostgresPool as Pool>::LabelledApi as LabelledPoolApi>::LabelledTransaction {
    let mut tx = postgres(deps)
        .with_rw("plan_storage_test", "hold_guard")
        .begin()
        .await
        .unwrap();
    DbGuards::<PostgresPool>::acquire_in_tx(&mut tx, &guards, Duration::from_secs(5))
        .await
        .unwrap();
    tx
}

/// A new member assigned while the edit waits for its guards is found on reread; the edit
/// rediscovers and checks the new member's subtree instead of committing.
pub async fn test_plan_edit_rediscovers_members_after_wait(deps: &Deps) {
    let repo = &deps.plan_storage_repo;
    let root = root(deps).await;
    let plan_id = define(deps, root.account_id, range(Some(1), 100)).await;
    let first = child(deps, root.account_id, &unique_segment(20)).await;
    assign(deps, first.account_id, plan_id).await;
    let late = child(deps, root.account_id, &unique_segment(20)).await;
    let late_child = child(
        deps,
        late.account_id,
        &AccountSegment::from_str("c").unwrap(),
    )
    .await;
    let late_plan = define(deps, late.account_id, range(None, 7)).await;
    assign(deps, late_child.account_id, late_plan).await;
    let definition = repo.get_definition(plan_id).await.unwrap().unwrap();

    let mut held = GuardSet::new();
    held.add(GuardClass::Account, root.account_id, GuardMode::Exclusive);
    let mut tx = hold_guard(deps, held).await;

    let editor = short_repo(deps, Duration::from_secs(10), 8);
    let edit = tokio::spawn({
        let name = definition.record.name.clone();
        async move {
            editor
                .edit_definition(plan_id, 0, name, range(Some(10), 100), Uuid::new_v4())
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!edit.is_finished());

    // The holder adds a member, as a concurrent assignment would, and commits.
    tx.execute(
        sqlx::query(
            "INSERT INTO account_plan_assignments \
             (account_id, parent_id, plan_id, revision, updated_at, modified_by) \
             VALUES ($1, $2, $3, 0, now(), $4)",
        )
        .bind(late.account_id)
        .bind(root.account_id)
        .bind(plan_id)
        .bind(Uuid::new_v4()),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let_assert!(Err(PlanStorageRepoError::Incompatible { account_id, .. }) = edit.await.unwrap());
    assert!(account_id == late_child.account_id);
    let current = repo.get_definition(plan_id).await.unwrap().unwrap();
    assert!(current.record.current_revision == 0);
}

/// A held guard on an unrelated branch does not delay a write; a held guard on a member times
/// out without partial effects, and a cancelled waiter leaves nothing behind.
pub async fn test_plan_guard_scope_timeout_and_cancellation(deps: &Deps) {
    let root = root(deps).await;
    let unrelated = child(deps, root.account_id, &unique_segment(20)).await;
    let member = child(deps, root.account_id, &unique_segment(20)).await;
    let plan_id = define(deps, root.account_id, range(None, 100)).await;
    assign(deps, member.account_id, plan_id).await;
    let plan_name = deps
        .plan_storage_repo
        .get_definition(plan_id)
        .await
        .unwrap()
        .unwrap()
        .record
        .name;
    let fast = short_repo(deps, Duration::from_millis(300), 1);

    let mut held = GuardSet::new();
    held.add(
        GuardClass::Account,
        unrelated.account_id,
        GuardMode::Exclusive,
    );
    let tx = hold_guard(deps, held).await;
    let start = Instant::now();
    fast.edit_definition(plan_id, 0, plan_name.clone(), range(None, 90), actor())
        .await
        .unwrap();
    info!(
        elapsed_ms = start.elapsed().as_millis() as u64,
        "edit beside an unrelated held guard"
    );
    tx.rollback().await.unwrap();

    let mut held = GuardSet::new();
    held.add(GuardClass::Account, member.account_id, GuardMode::Shared);
    let tx = hold_guard(deps, held).await;
    let_assert!(
        Err(PlanStorageRepoError::Contention) = fast
            .edit_definition(plan_id, 1, plan_name.clone(), range(None, 80), actor())
            .await
    );
    let patient = short_repo(deps, Duration::from_secs(10), 1);
    let cancelled = tokio::time::timeout(
        Duration::from_millis(300),
        patient.edit_definition(plan_id, 1, plan_name.clone(), range(None, 70), actor()),
    )
    .await;
    assert!(cancelled.is_err());
    tx.rollback().await.unwrap();

    // Neither the timed-out nor the cancelled attempt committed; the expected revision is still 1.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let definition = deps
        .plan_storage_repo
        .get_definition(plan_id)
        .await
        .unwrap()
        .unwrap();
    assert!(definition.record.current_revision == 1);
    assert!(definition.policy == range(None, 90));
    patient
        .edit_definition(plan_id, 1, plan_name, range(None, 60), actor())
        .await
        .unwrap();
}

/// Colliding keys merge before acquisition, so one transaction never waits for itself, and
/// a collision between two transactions only adds contention.
pub async fn test_plan_guard_key_collisions(deps: &Deps) {
    let key = guard_key(GuardClass::Account, Uuid::new_v4());
    let mut colliding = GuardSet::new();
    colliding
        .add_key(GuardClass::Account, key, GuardMode::Shared)
        .add_key(GuardClass::Account, key, GuardMode::Exclusive)
        .add_key(GuardClass::Account, key, GuardMode::Shared);
    assert!(colliding.len() == 1);
    let tx = hold_guard(deps, colliding.clone()).await;

    let pool = postgres(deps).clone();
    let waiter = tokio::spawn(async move {
        let start = Instant::now();
        pool.with_tx_err("plan_storage_test", "colliding_waiter", |tx| {
            async move {
                DbGuards::<PostgresPool>::acquire_in_tx(tx, &colliding, Duration::from_secs(10))
                    .await
            }
            .boxed()
        })
        .await
        .unwrap();
        start.elapsed()
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!waiter.is_finished());
    tx.commit().await.unwrap();
    let waited = waiter.await.unwrap();
    assert!(waited >= Duration::from_millis(300));
}
