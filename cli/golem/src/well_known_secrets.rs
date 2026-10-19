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

//! Protection against serving well-known secrets on a reachable address.
//!
//! The local development configuration uses secrets that are public:
//! [`golem_client::LOCAL_WELL_KNOWN_TOKEN`] as the admin token, and the compiled-in default keys
//! that sign webhook URLs and invocation session tokens. A listener on an address that is not
//! loopback gives anyone who can reach that address admin access, or lets them forge webhook
//! callbacks and invocation session tokens. Every listener counts, not only the one that checks
//! a given secret, because which listener serves which API is not a stable property.

use crate::config::GolemServerConfig;
use anyhow::{Context, anyhow, bail};
use golem_client::LOCAL_WELL_KNOWN_TOKEN;
use golem_common::model::auth::Token;
use golem_worker_executor::services::golem_config::AgentWebhooksServiceConfig;
use golem_worker_service::config::{InvocationSessionTokenConfig, WebhookCallbackHandlerConfig};
use poem::http::{Method, StatusCode, header};
use poem::middleware::CookieJarManager;
use poem::{Endpoint, EndpointExt, Request};
use std::net::IpAddr;

/// Names of the initial accounts whose configured token is the well-known local admin token.
pub fn accounts_with_well_known_token(config: &GolemServerConfig) -> Vec<&str> {
    let mut accounts: Vec<&str> = config
        .registry_service
        .initial_accounts
        .iter()
        .filter(|(_, account)| {
            account
                .token
                .as_ref()
                .is_some_and(|token| token.secret() == LOCAL_WELL_KNOWN_TOKEN)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    accounts.sort_unstable();
    accounts
}

/// The config keys that hold a well-known secret.
pub fn keys_with_well_known_secrets(config: &GolemServerConfig) -> Vec<String> {
    let mut keys: Vec<String> = accounts_with_well_known_token(config)
        .into_iter()
        .map(|account| format!("registry_service.initial_accounts.{account}.token"))
        .collect();
    if config.worker_service.webhook_callback_handler.hmac_key
        == WebhookCallbackHandlerConfig::default().hmac_key
    {
        keys.push("worker_service.webhook_callback_handler.hmac_key".to_string());
    }
    if config.worker_executor.agent_webhooks_service.hmac_key
        == AgentWebhooksServiceConfig::default().hmac_key
    {
        keys.push("worker_executor.agent_webhooks_service.hmac_key".to_string());
    }
    let default_session_token_key = InvocationSessionTokenConfig::default().active_key.key;
    let session_tokens = &config.worker_service.invocation_session_tokens;
    if session_tokens.active_key.key == default_session_token_key {
        keys.push("worker_service.invocation_session_tokens.active_key.key".to_string());
    }
    if session_tokens
        .verify_only_keys
        .iter()
        .any(|key| key.key == default_session_token_key)
    {
        keys.push("worker_service.invocation_session_tokens.verify_only_keys".to_string());
    }
    keys
}

/// Fails when a listener binds an address that is not loopback while the configuration holds a
/// well-known secret.
pub fn check_exposure(config: &GolemServerConfig) -> anyhow::Result<()> {
    let mut exposed = Vec::new();
    for (key, address) in config.listener_addresses() {
        let ip: IpAddr = address
            .parse()
            .with_context(|| format!("Invalid address in `{key}`: {address}"))?;
        if !ip.is_loopback() {
            exposed.push(format!("  {key} = {address}"));
        }
    }
    if exposed.is_empty() {
        return Ok(());
    }

    let keys = keys_with_well_known_secrets(config);
    if !keys.is_empty() {
        let settings = keys
            .iter()
            .map(|key| {
                format!(
                    "  {key} (environment variable GOLEM__{})",
                    key.replace('.', "__").to_uppercase()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "Refusing to listen on addresses that are not loopback addresses:\n{}\nbecause \
             these settings still have their well-known default values, which are \
             public:\n{settings}\nSet them to secret values, or bind every listener to a \
             loopback address such as 127.0.0.1. A token has at least 16 characters, each an \
             ASCII letter, a digit, `-` or `_`. A key is base64, for example the output of \
             `openssl rand -base64 32`, and the two webhook keys must be equal",
            exposed.join("\n")
        );
    }
    Ok(())
}

/// Deletes the well-known local admin token from the registry, if it exists there.
///
/// The registry only ever adds the tokens of the initial accounts, so a database that was once
/// used with the local development configuration keeps accepting the well-known token after the
/// configuration changes to a secret one. `registry_endpoint` is the registry's HTTP API, which
/// this calls in-process before the router exposes it. Returns whether a token was deleted.
pub async fn revoke_well_known_token(registry_endpoint: impl Endpoint) -> anyhow::Result<bool> {
    let endpoint = registry_endpoint.with(CookieJarManager::new());
    let request = |method: Method, path: &str| {
        Request::builder()
            .method(method)
            .uri_str(path)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {LOCAL_WELL_KNOWN_TOKEN}"),
            )
            .finish()
    };

    let response = endpoint
        .get_response(request(Method::GET, "/v1/me/token"))
        .await;
    match response.status() {
        StatusCode::UNAUTHORIZED => return Ok(false),
        status if status.is_success() => {}
        status => bail!("Failed to look up the well-known local admin token: {status}"),
    }
    let token: Token = response
        .into_body()
        .into_json()
        .await
        .map_err(|err| anyhow!("Failed to decode the well-known local admin token: {err}"))?;

    let response = endpoint
        .get_response(request(Method::DELETE, &format!("/v1/tokens/{}", token.id)))
        .await;
    if !response.status().is_success() {
        bail!(
            "Failed to delete the well-known local admin token: {}",
            response.status()
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::auth::TokenSecret;
    use golem_common::model::base64::Base64;
    use std::path::Path;
    use test_r::test;

    const SECRET_TOKEN: &str = "a-secret-admin-token-of-sufficient-length";

    fn config(router_address: &str) -> GolemServerConfig {
        let mut config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
        config.router.address = router_address.to_string();
        config
    }

    fn with_secrets(mut config: GolemServerConfig) -> GolemServerConfig {
        config
            .registry_service
            .initial_accounts
            .get_mut("root")
            .unwrap()
            .token = Some(TokenSecret::trusted(SECRET_TOKEN.to_string()));
        config.worker_service.webhook_callback_handler.hmac_key = Base64(vec![1; 32]);
        config.worker_executor.agent_webhooks_service.hmac_key = Base64(vec![1; 32]);
        config
            .worker_service
            .invocation_session_tokens
            .active_key
            .key = Base64(vec![2; 32]);
        config
    }

    #[test]
    fn well_known_secrets_are_served_on_loopback_only() {
        for address in ["127.0.0.1", "127.0.0.2", "::1"] {
            assert!(check_exposure(&config(address)).is_ok(), "{address}");
        }
        for address in ["0.0.0.0", "::", "192.168.1.10", "::ffff:127.0.0.1"] {
            let message = check_exposure(&config(address)).unwrap_err().to_string();
            for variable in [
                "GOLEM__REGISTRY_SERVICE__INITIAL_ACCOUNTS__ROOT__TOKEN",
                "GOLEM__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY",
                "GOLEM__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__HMAC_KEY",
                "GOLEM__WORKER_SERVICE__INVOCATION_SESSION_TOKENS__ACTIVE_KEY__KEY",
            ] {
                assert!(message.contains(variable), "{address}: {message}");
            }
            assert!(check_exposure(&with_secrets(config(address))).is_ok());
        }
    }

    #[test]
    fn every_listener_counts_not_only_the_router() {
        type SetAddress = fn(&mut GolemServerConfig, IpAddr);
        let listeners: [(&str, SetAddress); 5] = [
            ("registry_service.bind_address", |config, ip| {
                config.registry_service.bind_address = ip
            }),
            ("shard_manager.bind_address", |config, ip| {
                config.shard_manager.bind_address = ip
            }),
            ("worker_service.bind_address", |config, ip| {
                config.worker_service.bind_address = ip
            }),
            ("worker_executor.bind_address", |config, ip| {
                config.worker_executor.bind_address = ip
            }),
            (
                "component_compilation_service.bind_address",
                |config, ip| config.component_compilation_service.bind_address = ip,
            ),
        ];
        for (key, set_address) in listeners {
            let mut exposed = config("127.0.0.1");
            set_address(&mut exposed, "0.0.0.0".parse().unwrap());

            let message = check_exposure(&exposed).unwrap_err().to_string();

            assert!(message.contains(&format!("{key} = 0.0.0.0")), "{message}");
            assert!(!message.contains("router.address"), "{message}");
            assert!(
                message.contains("GOLEM__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY"),
                "{message}"
            );
            assert!(check_exposure(&with_secrets(exposed)).is_ok(), "{key}");
        }
    }

    #[test]
    fn each_well_known_secret_is_detected() {
        let secret = with_secrets(config("0.0.0.0"));
        assert!(keys_with_well_known_secrets(&secret).is_empty());

        let mut no_token = secret.clone();
        no_token
            .registry_service
            .initial_accounts
            .get_mut("root")
            .unwrap()
            .token = None;
        assert!(check_exposure(&no_token).is_ok());

        let mut other_account = secret.clone();
        other_account
            .registry_service
            .initial_accounts
            .get_mut("builtin_tool_owner")
            .unwrap()
            .token = Some(TokenSecret::trusted(LOCAL_WELL_KNOWN_TOKEN.to_string()));
        assert_eq!(
            accounts_with_well_known_token(&other_account),
            vec!["builtin_tool_owner"]
        );
        assert_eq!(
            keys_with_well_known_secrets(&other_account),
            vec!["registry_service.initial_accounts.builtin_tool_owner.token"]
        );

        let mut verify_only = secret.clone();
        verify_only
            .worker_service
            .invocation_session_tokens
            .verify_only_keys
            .push(InvocationSessionTokenConfig::default().active_key);
        assert_eq!(
            keys_with_well_known_secrets(&verify_only),
            vec!["worker_service.invocation_session_tokens.verify_only_keys"]
        );
        assert!(check_exposure(&verify_only).is_err());
    }

    #[test]
    fn invalid_router_address_is_rejected() {
        assert!(check_exposure(&config("localhost")).is_err());
    }
}
