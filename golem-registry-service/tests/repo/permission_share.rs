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

use super::Deps;
use super::sqlite::SqliteDb;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::card::{CardId, CardManagedBy, CardManagedByPermissionShare};
use golem_common::model::permission_share::{
    PermissionShareCreation, PermissionShareData, PermissionShareId, PermissionShareName,
};
use golem_registry_service::repo::card::{CardRepo, DbCardRepo};
use golem_registry_service::repo::model::audit::DeletableRevisionAuditFields;
use golem_registry_service::repo::model::card::CardRecord;
use golem_registry_service::repo::model::permission_share::{
    PermissionShareRepoError, PermissionShareRevisionRecord,
};
use golem_registry_service::repo::permission_share::{DbPermissionShareRepo, PermissionShareRepo};
use golem_registry_service::repo::registry_change::{ChangeEventId, RegistryChangeEvent};
use golem_registry_service::services::permission_share::{
    PermissionShareError, PermissionShareService,
};
use golem_registry_service::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use golem_service_base::db::PoolApi;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

fn card(id: PermissionShareId) -> CardRecord {
    CardRecord::creation(
        CardId::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
        true,
        Some(CardManagedBy::PermissionShare(
            CardManagedByPermissionShare {
                permission_share_id: id.0,
            },
        )),
    )
}

fn revision(owner: Uuid) -> PermissionShareRevisionRecord {
    PermissionShareRevisionRecord::creation(
        PermissionShareId::new(),
        PermissionShareName("shared-permissions".into()),
        PermissionShareData {
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        },
        AccountId(owner),
    )
}

async fn cursor(deps: &Deps) -> ChangeEventId {
    deps.registry_change_repo
        .get_latest_event_id()
        .await
        .unwrap()
        .unwrap_or(ChangeEventId(0))
}

fn assert_rejected_invalidation<T>(result: Result<T, PermissionShareRepoError>) {
    match result {
        Err(PermissionShareRepoError::InternalError(error)) => {
            assert!(format!("{error:#}").contains("reject invalidation"));
        }
        Err(error) => panic!("Expected invalidation insertion failure, got {error}"),
        Ok(_) => panic!("Expected invalidation insertion failure"),
    }
}

async fn assert_invalidation(
    deps: &Deps,
    baseline: ChangeEventId,
    target: Uuid,
    revoked_card: Option<Uuid>,
) {
    let events = deps
        .registry_change_repo
        .get_events_since(baseline)
        .await
        .unwrap();
    let invalidated: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RegistryChangeEvent::AccountTokensInvalidated { account_id, .. } => Some(*account_id),
            _ => None,
        })
        .collect();
    assert_eq!(invalidated, vec![target]);
    if let Some(card_id) = revoked_card {
        assert!(events.iter().any(|event| matches!(
            event,
            RegistryChangeEvent::CardRevoked { card_ids, .. } if card_ids.contains(&card_id)
        )));
    } else {
        assert_eq!(events.len(), 1);
    }
}

pub async fn mutations_invalidate_target_authentication(deps: &Deps, db: &SqliteDb) {
    let owner = deps.create_account().await.revision.account_id;
    let target = deps.create_account().await.revision.account_id;
    let repo = DbPermissionShareRepo::logged(db.pool.clone());
    let cards = DbCardRepo::new(db.pool.clone());
    let notifier = deps.test_registry_change_notifier();
    let initial = revision(owner);
    let id = PermissionShareId(initial.permission_share_id);

    let baseline = cursor(deps).await;
    let created = repo
        .create(owner, target, initial, card(id))
        .await
        .unwrap()
        .signal_new_events_available(&notifier);
    assert_invalidation(deps, baseline, target, None).await;
    assert_eq!(repo.get_by_id(id.0).await.unwrap().unwrap().share, created);

    let baseline = cursor(deps).await;
    let old_card = created.revision.card_id.unwrap();
    let mut update = created.revision;
    update.revision_id += 1;
    update.name = "updated-permissions".into();
    update.audit = DeletableRevisionAuditFields::new(owner);
    let updated = repo
        .update(update, card(id))
        .await
        .unwrap()
        .signal_new_events_available(&notifier);
    assert_invalidation(deps, baseline, target, Some(old_card)).await;
    assert!(cards.get(CardId(old_card)).await.unwrap().is_none());
    assert_eq!(repo.get_by_id(id.0).await.unwrap().unwrap().share, updated);

    let baseline = cursor(deps).await;
    let old_card = updated.revision.card_id.unwrap();
    let mut deletion = updated.revision;
    deletion.revision_id += 1;
    deletion.audit = DeletableRevisionAuditFields::deletion(owner);
    let deleted = repo
        .delete(deletion)
        .await
        .unwrap()
        .signal_new_events_available(&notifier);
    assert!(deleted.revision.audit.deleted);
    assert_invalidation(deps, baseline, target, Some(old_card)).await;
    assert!(cards.get(CardId(old_card)).await.unwrap().is_none());
    assert!(repo.get_by_id(id.0).await.unwrap().is_none());
}

pub async fn invalidation_failure_rolls_back_mutations(deps: &Deps, db: &SqliteDb) {
    let owner = deps.create_account().await.revision.account_id;
    let target = deps.create_account().await.revision.account_id;
    let repo = DbPermissionShareRepo::logged(db.pool.clone());
    let cards = DbCardRepo::new(db.pool.clone());
    let notifier = deps.test_registry_change_notifier();
    let initial = revision(owner);
    let id = PermissionShareId(initial.permission_share_id);
    let created = repo
        .create(owner, target, initial, card(id))
        .await
        .unwrap()
        .signal_new_events_available(&notifier);
    let baseline = cursor(deps).await;

    db.pool
        .with_rw("test", "reject_invalidation")
        .execute(sqlx::query(
            "CREATE TRIGGER reject_share_invalidation BEFORE INSERT ON registry_change_events
             WHEN NEW.event_type = 1
             BEGIN SELECT RAISE(ABORT, 'reject invalidation'); END",
        ))
        .await
        .unwrap();

    let mut another = revision(owner);
    another.name = "another-share".into();
    let another_id = PermissionShareId(another.permission_share_id);
    let another_card = card(another_id);
    assert_rejected_invalidation(
        repo.create(owner, target, another, another_card.clone())
            .await,
    );
    assert!(repo.get_by_id(another_id.0).await.unwrap().is_none());
    assert!(
        cards
            .get(CardId(another_card.card_id))
            .await
            .unwrap()
            .is_none()
    );

    let mut update = created.revision.clone();
    update.revision_id += 1;
    update.name = "updated-permissions".into();
    update.audit = DeletableRevisionAuditFields::new(owner);
    let replacement = card(id);
    assert_rejected_invalidation(repo.update(update, replacement.clone()).await);
    assert!(
        cards
            .get(CardId(replacement.card_id))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(repo.get_by_id(id.0).await.unwrap().unwrap().share, created);

    let mut deletion = created.revision.clone();
    deletion.revision_id += 1;
    deletion.audit = DeletableRevisionAuditFields::deletion(owner);
    assert_rejected_invalidation(repo.delete(deletion).await);
    assert_eq!(repo.get_by_id(id.0).await.unwrap().unwrap().share, created);
    assert!(
        cards
            .get(CardId(created.revision.card_id.unwrap()))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        deps.registry_change_repo
            .get_events_since(baseline)
            .await
            .unwrap()
            .is_empty()
    );

    db.pool
        .with_rw("test", "allow_invalidation")
        .execute(sqlx::query("DROP TRIGGER reject_share_invalidation"))
        .await
        .unwrap();
}

struct CountingNotifier {
    signals: AtomicUsize,
    sender: tokio::sync::broadcast::Sender<RegistryChangeEvent>,
}

impl RegistryChangeNotifier for CountingNotifier {
    fn signal_new_events_available(&self) {
        self.signals.fetch_add(1, Ordering::SeqCst);
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<RegistryChangeEvent> {
        self.sender.subscribe()
    }
}

pub async fn creation_signals_only_after_success(deps: &Deps, db: &SqliteDb) {
    let owner = deps.create_account().await.revision.account_id;
    let target = deps.create_account().await;
    let notifier = Arc::new(CountingNotifier {
        signals: AtomicUsize::new(0),
        sender: tokio::sync::broadcast::channel(1).0,
    });
    let service = PermissionShareService::new(
        Arc::new(DbPermissionShareRepo::logged(db.pool.clone())),
        deps.account_service(),
        notifier.clone(),
    );
    let creation = PermissionShareCreation {
        target_account_email: AccountEmail::new(target.revision.email),
        name: PermissionShareName("notified-share".into()),
        data: PermissionShareData {
            lower_positive: Vec::new(),
            lower_negative: Vec::new(),
            upper_positive: Vec::new(),
            upper_negative: Vec::new(),
        },
    };
    let baseline = cursor(deps).await;
    service
        .create(AccountId(owner), creation.clone(), &AuthCtx::System)
        .await
        .unwrap();
    assert_eq!(notifier.signals.load(Ordering::SeqCst), 1);
    assert_invalidation(deps, baseline, target.revision.account_id, None).await;

    let baseline = cursor(deps).await;
    assert!(matches!(
        service
            .create(AccountId(owner), creation, &AuthCtx::System)
            .await,
        Err(PermissionShareError::PermissionShareAlreadyExists)
    ));
    assert_eq!(notifier.signals.load(Ordering::SeqCst), 1);
    assert_eq!(cursor(deps).await, baseline);
}
