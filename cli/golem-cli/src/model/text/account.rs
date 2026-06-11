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

use crate::model::cli_output::CliOutput;
use crate::model::text::fmt::*;
use golem_client::model::{Account, PermissionShare};
use golem_common::model::account::AccountId;
use golem_common::model::permission_share::{PermissionShareData, PermissionShareId};
use serde::{Deserialize, Serialize};

fn account_fields(account: &Account) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Account ID", &account.id, format_main_id)
        .fmt_field("E-mail", &account.email, format_id)
        .field("Name", &account.name);

    fields.build()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountGetView(pub Account);

impl MessageWithFields for AccountGetView {
    fn message(&self) -> String {
        format!(
            "Got metadata for account {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl CliOutput for AccountGetView {
    const KIND: &'static str = "account.get.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountNewView(pub Account);

impl MessageWithFields for AccountNewView {
    fn message(&self) -> String {
        format!(
            "Created new account {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl CliOutput for AccountNewView {
    const KIND: &'static str = "account.new.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountUpdateView(pub Account);

impl MessageWithFields for AccountUpdateView {
    fn message(&self) -> String {
        format!("Updated account {}", format_message_highlight(&self.0.id))
    }

    fn fields(&self) -> Vec<(String, String)> {
        account_fields(&self.0)
    }
}

impl CliOutput for AccountUpdateView {
    const KIND: &'static str = "account.update.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountDeleteResult {
    pub deleted: bool,
    pub account_id: AccountId,
}

impl TextView for AccountDeleteResult {
    fn log(&self) {}
}

impl CliOutput for AccountDeleteResult {
    const KIND: &'static str = "account.delete.result";
}

fn permission_share_fields(share: &PermissionShare) -> Vec<(String, String)> {
    let mut fields = FieldsBuilder::new();

    fields
        .fmt_field("Permission share ID", &share.id, format_main_id)
        .field("Name", &share.name)
        .field("Revision", &share.revision)
        .fmt_field("Owner account ID", &share.owner_account_id, format_id)
        .fmt_field("Target account ID", &share.target_account_id, format_id)
        .field("Lower positive", &format_grants(&share.data.lower_positive))
        .field("Lower negative", &format_grants(&share.data.lower_negative));

    fields.build()
}

fn format_grants(grants: &[String]) -> String {
    if grants.is_empty() {
        "(none)".to_string()
    } else {
        grants.join("\n")
    }
}

fn grant_count(data: &PermissionShareData) -> usize {
    data.lower_positive.len() + data.lower_negative.len()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareGetView(pub PermissionShare);

impl MessageWithFields for PermissionShareGetView {
    fn message(&self) -> String {
        format!(
            "Got permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl CliOutput for PermissionShareGetView {
    const KIND: &'static str = "account.permission-share.get.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareNewView(pub PermissionShare);

impl MessageWithFields for PermissionShareNewView {
    fn message(&self) -> String {
        format!(
            "Created permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl CliOutput for PermissionShareNewView {
    const KIND: &'static str = "account.permission-share.new.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareUpdateView(pub PermissionShare);

impl MessageWithFields for PermissionShareUpdateView {
    fn message(&self) -> String {
        format!(
            "Updated permission share {}",
            format_message_highlight(&self.0.id)
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        permission_share_fields(&self.0)
    }
}

impl CliOutput for PermissionShareUpdateView {
    const KIND: &'static str = "account.permission-share.update.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareDeleteResult {
    pub deleted: bool,
    pub permission_share_id: PermissionShareId,
}

impl TextView for PermissionShareDeleteResult {
    fn log(&self) {}
}

impl CliOutput for PermissionShareDeleteResult {
    const KIND: &'static str = "account.permission-share.delete.result";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionShareListView(pub Vec<PermissionShare>);

impl TextView for PermissionShareListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("ID"),
            Column::new("Name"),
            Column::new("Owner").fixed(),
            Column::new("Target").fixed(),
            Column::new("Grants").fixed(),
        ]);

        for share in &self.0 {
            table.add_row(vec![
                share.id.to_string(),
                share.name.to_string(),
                share.owner_account_id.to_string(),
                share.target_account_id.to_string(),
                grant_count(&share.data).to_string(),
            ]);
        }

        log_table(table);
    }
}

impl CliOutput for PermissionShareListView {
    const KIND: &'static str = "account.permission-share.list.result";
}

// TODO: atomic
/*
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct GrantGetView(pub Vec<Role>);

impl TextView for GrantGetView {
    fn log(&self) {
        if self.0.is_empty() {
            logln("No roles granted")
        } else {
            logln("Granted roles:");
            for role in &self.0 {
                logln(format!("  - {role}"));
            }
        }
    }
}
*/
