// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use super::{MANIFEST, registry_archive, write_tar};
use anyhow::{Context, ensure};
use bytes::Bytes;
use docker_credential::{CredentialRetrievalError, DockerCredential};
use futures_util::{Stream, TryStreamExt};
use oci_spec::distribution::Reference;
use reqwest::{Client, RequestBuilder, Response, StatusCode, header};
use sha2::{Digest, Sha256};
use std::time::Duration;
use url::{Host, Url};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);

pub(super) fn package_url(source: &str) -> anyhow::Result<Url> {
    let url = Url::parse(source).map_err(|_| anyhow::anyhow!("Invalid package URL"))?;
    ensure!(
        url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Package URLs must have a host and no credentials, query string, or fragment"
    );
    ensure!(
        secure_or_loopback(&url),
        "Package URLs must use HTTPS except on loopback"
    );
    Ok(url)
}

fn loopback(url: &Url) -> bool {
    url.host().is_some_and(|host| match host {
        Host::Domain(host) => host == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    })
}

fn secure_or_loopback(url: &Url) -> bool {
    url.scheme() == "https" || (url.scheme() == "http" && loopback(url))
}

pub(super) async fn fetch_url(url: Url, limit: u64) -> anyhow::Result<Vec<u8>> {
    let fetch = async {
        let direct = loopback(&url);
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if !secure_or_loopback(attempt.url())
                    || (attempt.url().scheme() == "http" && !direct)
                    || !attempt.url().username().is_empty()
                    || attempt.url().password().is_some()
                    || attempt.previous().len() >= 10
                {
                    attempt.error("Unsafe package URL redirect")
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT);
        let client = if direct { client.no_proxy() } else { client }.build()?;
        let response = client
            .get(url.clone())
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        response_bytes(response, limit).await
    };
    tokio::time::timeout(FETCH_TIMEOUT, fetch)
        .await
        .context("Package download deadline exceeded")
        .and_then(|result| result)
        .with_context(|| format!("Cannot fetch {url} (anonymous HTTP; no credential key)"))
}

pub(super) fn oci_reference(source: &str, digest: Option<&str>) -> anyhow::Result<Reference> {
    let reference: Reference = source
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid OCI package reference"))?;
    if let Some(embedded) = reference.digest() {
        ensure!(
            digest.is_none_or(|digest| embedded == digest),
            "OCI source digest disagrees with the package pin"
        );
    }
    Ok(match digest {
        Some(digest) => reference.clone_with_digest(digest.to_owned()),
        None => reference,
    })
}

pub(super) async fn fetch_oci(reference: &Reference, limit: u64) -> anyhow::Result<Vec<u8>> {
    let registry = reference.resolve_registry();
    let credential_key = if reference.registry() == "docker.io" {
        "https://index.docker.io/v1/"
    } else {
        registry
    };
    let fetch = async {
        let base = Url::parse(&format!("https://{registry}"))?;
        let loopback = loopback(&base);
        let base = if loopback {
            Url::parse(&format!("http://{registry}"))?
        } else {
            base
        };
        let client = Client::builder()
            .https_only(!loopback)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT);
        let client = if loopback {
            client
                .no_proxy()
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if !secure_or_loopback(attempt.url()) || attempt.previous().len() >= 10 {
                        attempt.error("Unsafe OCI registry redirect")
                    } else {
                        attempt.follow()
                    }
                }))
        } else {
            client
        }
        .build()?;
        let repo = format!("v2/{}", reference.repository());
        let target = reference
            .digest()
            .or_else(|| reference.tag())
            .unwrap_or("latest");
        let manifest_url = base.join(&format!("{repo}/manifests/{target}"))?;
        let request = || {
            client
                .get(manifest_url.clone())
                .header(header::ACCEPT, MANIFEST)
        };
        let response = request()
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        let auth = if response.status() == StatusCode::UNAUTHORIZED {
            ensure!(
                !loopback,
                "HTTP loopback registries must allow anonymous pulls"
            );
            ensure!(
                response.url().origin() == base.origin(),
                "Unexpected OCI authentication challenge from a redirect"
            );
            Some(
                registry_auth(
                    &client,
                    response.headers(),
                    credential_key,
                    reference.repository(),
                )
                .await?,
            )
        } else {
            None
        };
        let response = match &auth {
            Some((auth, _)) => auth
                .apply(request())
                .send()
                .await
                .map_err(reqwest::Error::without_url)?,
            None => response,
        };
        let manifest = with_credential_hint(
            response_bytes(response, limit.min(4 * 1024 * 1024)).await,
            auth.as_ref().and_then(|(_, hint)| *hint),
        )?;
        if let Some(digest) = reference.digest() {
            ensure!(
                digest == format!("sha256:{:x}", Sha256::digest(&manifest)),
                "OCI manifest digest mismatch"
            );
        }
        let (mut entries, blobs) = registry_archive(&manifest)?;
        for (path, size) in blobs {
            let hex = path.strip_prefix("blobs/sha256/").unwrap();
            let digest = format!("sha256:{hex}");
            let request = client.get(base.join(&format!("{repo}/blobs/{digest}"))?);
            let request = match &auth {
                Some((auth, _)) => auth.apply(request),
                None => request,
            };
            let response = request.send().await.map_err(reqwest::Error::without_url)?;
            let bytes = with_credential_hint(
                response_bytes(response, size).await,
                auth.as_ref().and_then(|(_, hint)| *hint),
            )?;
            ensure!(bytes.len() as u64 == size, "Size mismatch for {digest}");
            entries.insert(path, bytes);
        }
        let archive = write_tar(&entries)?;
        ensure!(archive.len() as u64 <= limit, "Package size limit exceeded");
        Ok(archive)
    };
    tokio::time::timeout(FETCH_TIMEOUT, fetch)
        .await
        .context("Package download deadline exceeded")
        .and_then(|result| result)
        .with_context(|| {
            format!(
                "Cannot fetch oci://{reference} (would use Docker credential key {credential_key})"
            )
        })
}

enum RegistryAuth {
    Basic(String, String),
    Bearer(String),
}

impl RegistryAuth {
    fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self {
            Self::Basic(user, password) => request.basic_auth(user, Some(password)),
            Self::Bearer(token) => request.bearer_auth(token),
        }
    }
}

async fn registry_auth(
    client: &Client,
    headers: &header::HeaderMap,
    credential_key: &str,
    repository: &str,
) -> anyhow::Result<(RegistryAuth, Option<&'static str>)> {
    let challenge = registry_challenge(headers)?;
    let key = credential_key.to_owned();
    // A stuck helper must not hold up Tokio's runtime shutdown after the fetch deadline.
    let (send, receive) = tokio::sync::oneshot::channel();
    std::thread::Builder::new().spawn(move || {
        let (credentials, hint) = registry_credentials(docker_credential::get_credential(&key));
        let _ = send.send((credentials, hint));
    })?;
    let (credentials, hint) = receive.await?;
    let result = match challenge {
        RegistryChallenge::Basic => {
            credentials.context("Registry requires a Docker username and password")
        }
        RegistryChallenge::Bearer { realm, service } => {
            registry_token(client, realm, service, repository, credentials).await
        }
    };
    Ok((with_credential_hint(result, hint)?, hint))
}

fn with_credential_hint<T>(result: anyhow::Result<T>, hint: Option<&str>) -> anyhow::Result<T> {
    match hint {
        Some(hint) => result.with_context(|| hint.to_owned()),
        None => result,
    }
}

fn registry_credentials(
    credentials: Result<DockerCredential, CredentialRetrievalError>,
) -> (Option<RegistryAuth>, Option<&'static str>) {
    match credentials {
        Ok(DockerCredential::UsernamePassword(user, password)) => {
            (Some(RegistryAuth::Basic(user, password)), None)
        }
        Ok(DockerCredential::IdentityToken(_)) => (
            None,
            Some(
                "Docker identity tokens are not supported; log in with a registry username and password or access token",
            ),
        ),
        Err(
            CredentialRetrievalError::ConfigNotFound
            | CredentialRetrievalError::ConfigReadError
            | CredentialRetrievalError::NoCredentialConfigured,
        ) => (None, None),
        // Helper errors can contain secrets. Give a fixed hint instead of the helper's output.
        Err(_) => (None, Some("Could not read Docker registry credentials")),
    }
}

enum RegistryChallenge {
    Basic,
    Bearer { realm: Url, service: Option<String> },
}

fn registry_challenge(headers: &header::HeaderMap) -> anyhow::Result<RegistryChallenge> {
    let challenges = headers
        .get_all(header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(http_auth::ChallengeParser::new)
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    let bearer = challenges
        .iter()
        .find(|challenge| challenge.scheme.eq_ignore_ascii_case("Bearer"));
    let Some(bearer) = bearer else {
        ensure!(
            challenges
                .iter()
                .any(|c| c.scheme.eq_ignore_ascii_case("Basic")),
            "Registry did not offer supported authentication"
        );
        return Ok(RegistryChallenge::Basic);
    };
    let param = |name| {
        bearer
            .params
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.to_unescaped())
    };
    let realm = Url::parse(&param("realm").context("Registry token endpoint is missing")?)?;
    ensure!(
        realm.scheme() == "https" && realm.username().is_empty() && realm.password().is_none(),
        "Registry token endpoint must use HTTPS without embedded credentials"
    );
    Ok(RegistryChallenge::Bearer {
        realm,
        service: param("service"),
    })
}

async fn registry_token(
    client: &Client,
    realm: Url,
    service: Option<String>,
    repository: &str,
    credentials: Option<RegistryAuth>,
) -> anyhow::Result<RegistryAuth> {
    let mut request = client
        .get(realm)
        .query(&[("scope", format!("repository:{repository}:pull"))]);
    if let Some(service) = service {
        request = request.query(&[("service", service)]);
    }
    if let Some(credentials) = credentials {
        request = credentials.apply(request);
    }
    let response = request.send().await.map_err(reqwest::Error::without_url)?;
    #[derive(serde::Deserialize)]
    struct Token {
        token: Option<String>,
        access_token: Option<String>,
    }
    let token: Token = serde_json::from_slice(&response_bytes(response, 64 * 1024).await?)
        .map_err(|_| anyhow::anyhow!("Registry returned an invalid token response"))?;
    Ok(RegistryAuth::Bearer(
        token
            .token
            .or(token.access_token)
            .filter(|token| !token.is_empty())
            .context("Registry returned no token")?,
    ))
}

async fn response_bytes(response: Response, limit: u64) -> anyhow::Result<Vec<u8>> {
    let response = response
        .error_for_status()
        .map_err(reqwest::Error::without_url)?;
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= limit),
        "Package download size limit exceeded"
    );
    collect(
        response.bytes_stream().map_err(reqwest::Error::without_url),
        limit,
    )
    .await
}

async fn collect<E>(
    stream: impl Stream<Item = Result<Bytes, E>>,
    limit: u64,
) -> anyhow::Result<Vec<u8>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    tokio::pin!(stream);
    let mut result = Vec::new();
    while let Some(chunk) = stream.try_next().await? {
        ensure!(
            chunk.len() as u64 <= limit - result.len() as u64,
            "Package size limit exceeded"
        );
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn registry_auth_challenges_prefer_secure_bearer_and_accept_basic() {
        let mut headers = header::HeaderMap::new();
        headers.append(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Basic realm=\"registry\""),
        );
        assert!(matches!(
            registry_challenge(&headers).unwrap(),
            RegistryChallenge::Basic
        ));
        headers.append(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static(
                "Bearer realm=\"https://tokens.example/token\",service=\"registry.example\"",
            ),
        );
        let RegistryChallenge::Bearer { realm, service } = registry_challenge(&headers).unwrap()
        else {
            panic!("Expected Bearer")
        };
        assert_eq!(realm.as_str(), "https://tokens.example/token");
        assert_eq!(service.as_deref(), Some("registry.example"));
        for challenge in [
            "Bearer realm=\"http://tokens.example/token\"",
            "Bearer realm=\"https://user:password@tokens.example/token\"",
        ] {
            headers.insert(
                header::WWW_AUTHENTICATE,
                header::HeaderValue::from_static(challenge),
            );
            assert!(registry_challenge(&headers).is_err());
        }
    }

    #[test]
    fn registry_credentials_keep_identity_and_helper_errors_out_of_requests() {
        let (auth, hint) = registry_credentials(Ok(DockerCredential::UsernamePassword(
            "user".into(),
            "password".into(),
        )));
        assert!(
            matches!(auth, Some(RegistryAuth::Basic(user, password)) if user == "user" && password == "password")
        );
        assert!(hint.is_none());
        let (auth, hint) =
            registry_credentials(Ok(DockerCredential::IdentityToken("never-sent".into())));
        assert!(auth.is_none());
        assert!(with_credential_hint(Ok(()), hint).is_ok());
        assert!(hint.unwrap().contains("identity tokens are not supported"));
        let (auth, hint) = registry_credentials(Err(CredentialRetrievalError::HelperFailure {
            helper: "helper".into(),
            stdout: "never-shown".into(),
            stderr: String::new(),
        }));
        assert!(auth.is_none());
        assert_eq!(hint, Some("Could not read Docker registry credentials"));
    }

    #[test]
    fn locked_oci_fetch_uses_digest_instead_of_tag() {
        let source = "ghcr.io/example/my-package:1.0";
        let digest = format!("sha256:{}", "a".repeat(64));
        let reference = oci_reference(source, Some(&digest)).unwrap();
        assert_eq!(
            reference.to_string(),
            format!("ghcr.io/example/my-package@{digest}")
        );
        assert!(
            oci_reference(
                &format!("{source}@sha256:{}", "b".repeat(64)),
                Some(&digest)
            )
            .is_err()
        );
    }

    #[test]
    async fn cold_oci_fetch_reports_the_source_and_credential_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let source = format!("{host}/test/package:1.0");
        let server = tokio::spawn(async move {
            let app = axum::Router::new()
                .fallback(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE });
            axum::serve(listener, app).await.unwrap();
        });
        let error = fetch_oci(&oci_reference(&source, None).unwrap(), 4096)
            .await
            .unwrap_err();
        server.abort();
        let message = format!("{error:#}");
        assert!(message.contains(&format!("oci://{source}")), "{message}");
        assert!(
            message.contains(&format!("Docker credential key {host}")),
            "{message}"
        );
    }
}
