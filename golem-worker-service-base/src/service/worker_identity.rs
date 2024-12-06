use async_trait::async_trait;
use golem_common::config::WorkerIdentityConfig;
use jsonwebkey::ByteArray;
use jsonwebkey::JsonWebKey;
use poem::web::Json;
// use openidconnect::jwk::{Jwk, RsaKey, EcKey, HmacKey};
use anyhow::anyhow;
use anyhow::Result;
use rsa::{pkcs1::DecodeRsaPublicKey, RsaPublicKey};

#[async_trait]
pub trait WorkerIdentityService {
    async fn get_jwks(&self) -> Result<Vec<jsonwebkey::JsonWebKey>>;
}

pub struct WorkerIdentityServiceDefault {
    config: WorkerIdentityConfig,
}

impl WorkerIdentityServiceDefault {
    pub fn new(config: WorkerIdentityConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl WorkerIdentityService for WorkerIdentityServiceDefault {
    async fn get_jwks(&self) -> Result<Vec<jsonwebkey::JsonWebKey>> {
        self.config
            .set
            .iter()
            .map(|a| {
                let der_encoded_key = &a.der;
                match &a.alg {
                    // "RS256" | "RS384" | "RS512" => {
                    //     // Parse RSA public key
                    //     let public_key = RsaPublicKey::from_pkcs1_der(&der_encoded_key)?;
                    //     let jwk = RsaKey::from(&public_key);
                    //     let jwk_json = serde_json::to_string(&jwk)?;
                    //     Ok(jwk_json)
                    // },
                    "ES256" => {
                        // Parse the DER-encoded private key into a SigningKey (which internally contains the private key 'd')
                        let signing_key = p256::SecretKey::from_sec1_der(&der_encoded_key)?;
                        let public_key = signing_key.public_key();
                        let pub_key_bytes = public_key.to_sec1_bytes();
                        let (x, y) = split_sec1_public_key(&pub_key_bytes).unwrap(); // P256 public keys are 64 bytes (x and y each 32 bytes)

                        let mut jwk = JsonWebKey::new(jsonwebkey::Key::EC {
                            curve: {
                                jsonwebkey::Curve::P256 {
                                    d: None,
                                    x: ByteArray::from_slice(x),
                                    y: ByteArray::from_slice(y),
                                }
                            },
                        });
                        jwk.key_id = Some(a.kid.clone());
                        let _ = jwk.set_algorithm(a.alg);

                        Ok(jwk)
                    }
                    // "HS256" | "HS384" | "HS512" => {
                    //     // For HMAC keys, you'd need a secret key (e.g., from a shared secret)
                    //     let hmac_key = HmacKey::from(&der_encoded_key);
                    //     let jwk_json = serde_json::to_string(&hmac_key)?;
                    //     Ok(jwk_json)
                    // },
                    _ => Err(anyhow!("Unsupported algorithm")), // Handle other algorithms or errors
                }
            })
            .collect()
    }
}

fn split_sec1_public_key(sec1_key: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    if sec1_key.len() != 65 || sec1_key[0] != 0x04 {
        return Err("Invalid SEC1 public key format".to_string());
    }

    // Extract x and y coordinates
    let x = sec1_key[1..33].to_vec(); // Bytes 1 to 32 (x-coordinate)
    let y = sec1_key[33..65].to_vec(); // Bytes 33 to 64 (y-coordinate)

    Ok((x, y))
}
