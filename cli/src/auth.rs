use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use golem_client::models::{OAuth2Data, Token, TokenSecret, UnsafeToken};
use indoc::printdoc;
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use crate::clients::login::LoginClient;
use crate::clients::CloudAuthentication;
use crate::model::GolemError;

#[async_trait]
pub trait Auth {
    async fn authenticate(
        &self,
        manual_token: Option<Uuid>,
        config_dir: PathBuf,
    ) -> Result<CloudAuthentication, GolemError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthLive<L: LoginClient + Send + Sync> {
    pub login: L,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudAuthenticationConfig {
    data: CloudAuthenticationConfigData,
    secret: Uuid,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudAuthenticationConfigData {
    id: Uuid,
    account_id: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl<L: LoginClient + Send + Sync> AuthLive<L> {
    fn read_from_file(&self, config_dir: &Path) -> Option<CloudAuthentication> {
        let file = File::open(self.config_path(config_dir)).ok()?; // TODO log

        let reader = BufReader::new(file);

        let parsed: serde_json::Result<CloudAuthenticationConfig> = serde_json::from_reader(reader);

        match parsed {
            Ok(conf) => Some(CloudAuthentication(UnsafeToken {
                data: Box::new(Token {
                    id: conf.data.id,
                    account_id: conf.data.account_id,
                    created_at: conf.data.created_at.to_string(),
                    expires_at: conf.data.expires_at.to_string(),
                }),
                secret: Box::new(TokenSecret { value: conf.secret }),
            })),
            Err(err) => {
                info!("Parsing failed: {err}"); // TODO configure
                None
            }
        }
    }

    fn config_path(&self, config_dir: &Path) -> PathBuf {
        config_dir.join("cloud_authentication.json")
    }

    fn store_file(&self, token: &UnsafeToken, config_dir: &Path) {
        match create_dir_all(config_dir) {
            Ok(_) => {}
            Err(err) => {
                info!("Can't create config directory: {err}");
            }
        }
        let file_res = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(self.config_path(config_dir));
        let file = match file_res {
            Ok(file) => file,
            Err(err) => {
                info!("Can't open file: {err}");
                return;
            }
        };
        let writer = BufWriter::new(file);
        let data = CloudAuthenticationConfig {
            data: CloudAuthenticationConfigData {
                id: token.data.id,
                account_id: token.data.account_id.clone(),
                created_at: token.data.created_at.parse().unwrap(),
                expires_at: token.data.expires_at.parse().unwrap(),
            },
            secret: token.secret.value,
        };
        let res = serde_json::to_writer_pretty(writer, &data);

        if let Err(err) = res {
            info!("File sawing error: {err}");
        }
    }

    async fn oauth2(&self, config_dir: &Path) -> Result<CloudAuthentication, GolemError> {
        let data = self.login.start_oauth2().await?;
        inform_user(&data);
        let token = self.login.complete_oauth2(data.encoded_session).await?;
        self.store_file(&token, config_dir);
        Ok(CloudAuthentication(token))
    }

    async fn config_authentication(
        &self,
        config_dir: PathBuf,
    ) -> Result<CloudAuthentication, GolemError> {
        if let Some(data) = self.read_from_file(&config_dir) {
            Ok(data)
        } else {
            self.oauth2(&config_dir).await
        }
    }
}

fn inform_user(data: &OAuth2Data) {
    let box_url_line = String::from_utf8(vec![b'-'; data.url.len() + 2]).unwrap();
    let box_code_line = String::from_utf8(vec![b'-'; data.user_code.len() + 2]).unwrap();
    let expires: DateTime<Utc> = data.expires.parse().unwrap();
    let expires_in = expires.signed_duration_since(Utc::now()).num_minutes();
    let expires_at = expires.format("%T");
    let url = &data.url;
    let user_code = &data.user_code;

    printdoc! {"
        >>
        >>  Application requests to perform OAuth2
        >>  authorization.
        >>
        >>  Visit following URL in a browser:
        >>
        >>   ┏{box_url_line}┓
        >>   ┃ {url} ┃
        >>   ┗{box_url_line}┛
        >>
        >>  And enter following code:
        >>
        >>   ┏{box_code_line}┓
        >>   ┃ {user_code} ┃
        >>   ┗{box_code_line}┛
        >>
        >>  Code will expire in {expires_in} minutes at {expires_at}.
        >>
        Waiting...
    "};
}

#[async_trait]
impl<L: LoginClient + Send + Sync> Auth for AuthLive<L> {
    async fn authenticate(
        &self,
        manual_token: Option<Uuid>,
        config_dir: PathBuf,
    ) -> Result<CloudAuthentication, GolemError> {
        if let Some(manual_token) = manual_token {
            let secret = TokenSecret {
                value: manual_token,
            };
            let data = self.login.token_details(secret.clone()).await?;

            Ok(CloudAuthentication(UnsafeToken {
                data: Box::new(data),
                secret: Box::new(secret),
            }))
        } else {
            self.config_authentication(config_dir).await
        }
    }
}
