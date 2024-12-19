use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Context};
use async_trait::async_trait;

use base64ct::Base64Url;
use golem_common::config::WorkerIdentityConfig;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair};
use serde::{Deserialize, Serialize};
// use rsa::{RsaPrivateKey, RsaPublicKey, pkcs1::{EncodeRsaPrivateKey, EncodeRsaPublicKey}};
use rand::rngs::OsRng;
use serde_json::json;

/// Service implementing a persistent key-value store
#[async_trait]
pub trait WorkerIdentityService {
    async fn sign(&self, claims: WorkerClaims) -> anyhow::Result<String>;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkerClaims {
    pub account_id: String,
    pub component_id: String,
    pub worker_name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String, // Issuer
    pub sub: String, // Subject
    pub aud: String, // Audience
    pub exp: usize,  // Expiration time (as a Unix timestamp)
    pub nbf: usize,  // Not before time (as a Unix timestamp)
    pub iat: usize,  // Issued at time (as a Unix timestamp)
    pub jti: String, // Unique identifier for the token
    #[serde(flatten)]
    pub worker: WorkerClaims,
}

impl Claims {
    pub fn from_worker(claims: WorkerClaims, config: &WorkerIdentityConfig) -> Self {
        use base64ct::Encoding;
        use rand::Rng;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs() as usize;

        Self {
            iss: config.issuer.clone(),
            sub: format!("urn:worker:{}/{}", claims.component_id, claims.worker_name),
            aud: config.audience.clone(),
            exp: now + 60,
            nbf: now,
            iat: now,
            jti: Base64Url::encode_string(&rand::thread_rng().gen::<[u8; 16]>()),
            worker: claims,
        }
    }
}

#[derive(Clone)]
pub struct DefaultWorkerIdentityService {
    config: WorkerIdentityConfig,
}

impl DefaultWorkerIdentityService {
    pub fn new(config: WorkerIdentityConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerIdentityService for DefaultWorkerIdentityService {
    async fn sign(&self, worker_claims: WorkerClaims) -> anyhow::Result<String> {
        let claims = Claims::from_worker(worker_claims, &self.config);

        let mut active_keys = self
            .config
            .set
            .iter()
            .filter(|s| self.config.active_keys.contains(&s.kid));

        let valid_key = active_keys.next().context("no valid keys found")?;

        let alg = match valid_key.alg.as_str() {
            "ES256" => Algorithm::ES256,
            "HS256" => Algorithm::HS256,
            _ => todo!(),
        };

        let key = match alg {
            Algorithm::ES256 => jsonwebtoken::EncodingKey::from_ec_der(&valid_key.der),
            Algorithm::HS256 => jsonwebtoken::EncodingKey::from_secret(&valid_key.der),
            _ => todo!(),
        };

        let mut header = jsonwebtoken::Header::new(alg);
        header.kid = Some(valid_key.kid.clone());

        let token =
            jsonwebtoken::encode(&header, &claims, &key).context("token generation failed")?;

        return Ok(token);
    }
}
