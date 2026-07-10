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

use crate::model::cli_output::StructuredOutput;
use crate::model::masking::Masked;
use crate::model::text::fmt::*;
use golem_client::model::{CardManagedBy, StoredCard};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardGetView(pub StoredCard);

impl Masked for CardGetView {}

impl MessageWithFields for CardGetView {
    fn message(&self) -> String {
        format!("Got card {}", format_message_highlight(card_id(&self.0)))
    }

    fn fields(&self) -> Vec<(String, String)> {
        card_fields(&self.0)
    }
}

impl StructuredOutput for CardGetView {
    const KIND: &'static str = "card.get";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardListView {
    pub cards: Vec<StoredCard>,
}

impl TextOutput for CardListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("ID"),
            Column::new("Kind").fixed(),
            Column::new("Parents").fixed(),
            Column::new("System").fixed(),
            Column::new("Managed by"),
            Column::new("Expires at").fixed(),
            Column::new("Grants").fixed(),
        ]);

        for card in &self.cards {
            table.add_row(vec![
                card_id(card).to_string(),
                card_kind(card).to_string(),
                card_parent_count(card).to_string(),
                card_system(card).to_string(),
                card_managed_by(card),
                card_expires_at(card),
                card_grant_count(card).to_string(),
            ]);
        }

        log_table(table);
    }
}

impl StructuredOutput for CardListView {
    const KIND: &'static str = "card.list";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardRevokeResult {
    pub revoked_card_ids: Vec<Uuid>,
}

impl TextOutput for CardRevokeResult {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![Column::new("Revoked card ID")]);

        for card_id in &self.revoked_card_ids {
            table.add_row(vec![card_id.to_string()]);
        }

        log_table(table);
    }
}

impl StructuredOutput for CardRevokeResult {
    const KIND: &'static str = "card.revoke";
}

fn card_fields(card: &StoredCard) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Card ID", &card_id(card), format_main_id)
        .field("Kind", &card_kind(card))
        .field("Parent IDs", &format_ids(card_parent_ids(card)))
        .field("System", &card_system(card))
        .field("Managed by", &card_managed_by(card))
        .field("Created at", &card_created_at(card))
        .field("Expires at", &card_expires_at(card))
        .field("Lower positive", &format_grants(card_lower_positive(card)))
        .field("Lower negative", &format_grants(card_lower_negative(card)))
        .field("Upper positive", &format_grants(card_upper_positive(card)))
        .field("Upper negative", &format_grants(card_upper_negative(card)));

    fields.build()
}

fn card_id(card: &StoredCard) -> &Uuid {
    match card {
        StoredCard::Concrete(card) => &card.card_id,
        StoredCard::Polymorphic(card) => &card.card_id,
    }
}

fn card_parent_ids(card: &StoredCard) -> &[Uuid] {
    match card {
        StoredCard::Concrete(card) => &card.parent_ids,
        StoredCard::Polymorphic(card) => &card.parent_ids,
    }
}

fn card_parent_count(card: &StoredCard) -> usize {
    card_parent_ids(card).len()
}

fn card_kind(card: &StoredCard) -> &'static str {
    match card {
        StoredCard::Concrete(_) => "Concrete",
        StoredCard::Polymorphic(_) => "Polymorphic",
    }
}

fn card_system(card: &StoredCard) -> bool {
    match card {
        StoredCard::Concrete(card) => card.system_card,
        StoredCard::Polymorphic(card) => card.system_card,
    }
}

fn card_managed_by(card: &StoredCard) -> String {
    match card {
        StoredCard::Concrete(card) => match &card.managed_by {
            Some(CardManagedBy::AccountRoot(managed_by)) => {
                format!("account root {}", managed_by.account_id)
            }
            Some(CardManagedBy::EnvironmentDefault(managed_by)) => {
                format!("environment default {}", managed_by.environment_id)
            }
            Some(CardManagedBy::PermissionShare(managed_by)) => {
                format!("permission share {}", managed_by.permission_share_id)
            }
            Some(CardManagedBy::AgentInitial(managed_by)) => format!(
                "agent initial {} rev {} {}",
                managed_by.component_id, managed_by.component_revision, managed_by.agent_type
            ),
            None => "(none)".to_string(),
        },
        StoredCard::Polymorphic(_) => "(not stored on polymorphic cards)".to_string(),
    }
}

fn card_created_at(card: &StoredCard) -> String {
    match card {
        StoredCard::Concrete(card) => card.created_at.to_rfc3339(),
        StoredCard::Polymorphic(card) => card.created_at.to_rfc3339(),
    }
}

fn card_expires_at(card: &StoredCard) -> String {
    match card {
        StoredCard::Concrete(card) => card
            .expires_at
            .map(|expires_at| expires_at.to_rfc3339())
            .unwrap_or_else(|| "never".to_string()),
        StoredCard::Polymorphic(card) => card
            .expires_at
            .map(|expires_at| expires_at.to_rfc3339())
            .unwrap_or_else(|| "never".to_string()),
    }
}

fn card_lower_positive(card: &StoredCard) -> &[String] {
    match card {
        StoredCard::Concrete(card) => &card.lower_positive,
        StoredCard::Polymorphic(card) => &card.lower_positive,
    }
}

fn card_lower_negative(card: &StoredCard) -> &[String] {
    match card {
        StoredCard::Concrete(card) => &card.lower_negative,
        StoredCard::Polymorphic(card) => &card.lower_negative,
    }
}

fn card_upper_positive(card: &StoredCard) -> &[String] {
    match card {
        StoredCard::Concrete(card) => &card.upper_positive,
        StoredCard::Polymorphic(card) => &card.upper_positive,
    }
}

fn card_upper_negative(card: &StoredCard) -> &[String] {
    match card {
        StoredCard::Concrete(card) => &card.upper_negative,
        StoredCard::Polymorphic(card) => &card.upper_negative,
    }
}

fn card_grant_count(card: &StoredCard) -> usize {
    card_lower_positive(card).len()
        + card_lower_negative(card).len()
        + card_upper_positive(card).len()
        + card_upper_negative(card).len()
}

fn format_ids(ids: &[Uuid]) -> String {
    if ids.is_empty() {
        "(none)".to_string()
    } else {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn format_grants(grants: &[String]) -> String {
    if grants.is_empty() {
        "(none)".to_string()
    } else {
        grants.join("\n")
    }
}
