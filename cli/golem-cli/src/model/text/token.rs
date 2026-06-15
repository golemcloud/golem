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

use colored::Colorize;
use golem_client::model::{Token, TokenWithSecret};
use golem_common::model::auth::TokenId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenNewView(pub TokenWithSecret);

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
            .fmt_field("Token ID", &self.0.id, format_main_id)
            .fmt_field("Account ID", &self.0.account_id, format_id)
            .field("Created at", &self.0.created_at)
            .field("Expires at", &self.0.expires_at)
            .fmt_field("Secret (SAVE THIS)", &self.0.secret.secret(), |s| {
                s.to_string().bold().red().to_string()
            });

        fields.build()
    }
}

impl CliOutput for TokenNewView {
    const KIND: &'static str = "api-token.new.result";
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenListView {
    pub tokens: Vec<Token>,
}

impl TextView for TokenListView {
    fn log(&self) {
        let mut table = new_table_full_condensed(vec![
            Column::new("ID"),
            Column::new("Created at").fixed(),
            Column::new("Expires at").fixed(),
            Column::new("Account").fixed(),
        ]);
        for token in &self.tokens {
            table.add_row(vec![
                token.id.0.to_string(),
                token.created_at.to_string(),
                token.expires_at.to_string(),
                token.account_id.to_string(),
            ]);
        }
        log_table(table);
    }
}

impl CliOutput for TokenListView {
    const KIND: &'static str = "api-token.list.result";
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenDeleteResult {
    pub deleted: bool,
    pub token_id: TokenId,
}

impl TextView for TokenDeleteResult {
    fn log(&self) {}
}

impl CliOutput for TokenDeleteResult {
    const KIND: &'static str = "api-token.delete.result";
}
