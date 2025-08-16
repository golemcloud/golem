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

use crate::model::text::fmt::*;
use chrono::{DateTime, Utc};
use cli_table::Table;
use colored::Colorize;
use golem_client::model::{Token, UnsafeToken};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenNewView(pub UnsafeToken);

impl MessageWithFields for TokenNewView {
    fn message(&self) -> String {
        format!(
            "Created new token\n{}",
            format_warn("Save this token secret, you can't get this data later!")
        )
    }

    fn fields(&self) -> Vec<(String, String)> {
        let mut fields = FieldsBuilder::new();

        fields
            .fmt_field("Token ID", &self.0.data.id, format_main_id)
            .fmt_field("Account ID", &self.0.data.id, format_id)
            .field("Created at", &self.0.data.created_at)
            .field("Expires at", &self.0.data.expires_at)
            .fmt_field("Secret (SAVE THIS)", &self.0.secret.value, |s| {
                s.to_string().bold().red().to_string()
            });

        fields.build()
    }
}

#[derive(Table)]
struct TokenTableView {
    #[table(title = "ID")]
    pub id: Uuid,
    #[table(title = "Created at")]
    pub created_at: DateTime<Utc>,
    #[table(title = "Expires at")]
    pub expires_at: DateTime<Utc>,
    #[table(title = "Account")]
    pub account_id: String,
}

impl From<&Token> for TokenTableView {
    fn from(value: &Token) -> Self {
        TokenTableView {
            id: value.id,
            created_at: value.created_at,
            expires_at: value.expires_at,
            account_id: value.account_id.to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenListView(pub Vec<Token>);

impl TextView for TokenListView {
    fn log(&self) {
        log_table::<_, TokenTableView>(&self.0);
    }
}
