use golem_service_base::api_tags::ApiTags;

use golem_worker_service_base::api::WorkerApiBaseError;

use jwk::JsonWebKey;
use payload::{Binary, PlainText};
use poem::{IntoResponse, Request, Response};

use poem_openapi::payload::Json;
use poem_openapi::*;
use registry::{MetaMediaType, MetaSchema};
use serde::{Deserialize, Serialize};

use crate::{service::worker_identity::WorkerIdentityService, WorkerService};

pub struct IdentityApi {
    pub worker_identity_service: WorkerIdentityService,
}

type Result<T> = std::result::Result<T, WorkerApiBaseError>;

#[OpenApi(prefix_path = "/", tag = ApiTags::Worker)]
impl IdentityApi {
    /// Launch a new worker.
    ///
    /// Creates a new worker. The worker initially is in `Idle`` status, waiting to be invoked.
    ///
    /// The parameters in the request are the following:
    /// - `name` is the name of the created worker. This has to be unique, but only for a given component
    /// - `args` is a list of strings which appear as command line arguments for the worker
    /// - `env` is a list of key-value pairs (represented by arrays) which appear as environment variables for the worker
    #[oai(
        path = "/.well-known/jwks.json",
        method = "get",
        operation_id = "get_jwks"
    )]
    async fn get_jwk(&self) -> Result<Json<Vec<JsonWebKey>>> {
        let keys = self
            .worker_identity_service
            .get_jwks()
            .await
            .unwrap()
            .iter()
            .map(|j| JsonWebKey::from(j))
            .collect();

        // let serialized = serde_json::to_string(&keys).unwrap();

        // poem::Response::builder()
        // .header("Content-Type", "application/json")
        // .body(serialized)

        Ok(Json(keys))
    }

    /// Delete a worker
    ///
    /// Interrupts and deletes an existing worker.
    #[oai(
        path = "/.well-known/openid-configuration",
        method = "get",
        operation_id = "get_oidc_configuration"
    )]
    async fn get_oidc_configuration(&self, req: &Request) -> Result<Json<OidcDiscovery>> {
        // Get the scheme (http or https)
        let scheme = req.scheme().as_str();

        // Get the host from the "Host" header
        let host = req.header("host").unwrap_or("<unknown host>"); // Default if Host is missing

        // Construct the base URL
        let base_url = format!("{scheme}://{host}");

        Ok(Json(OidcDiscovery {
            issuer: base_url.clone(), // Base URL of your OIDC provider
            authorization_endpoint: format!("{}/unused", base_url),
            token_endpoint: format!("{}/unused", base_url),
            jwks_uri: format!("{}/.well-known/jwks.json", base_url),
            response_types_supported: vec![
                "code".to_string(),
                "token".to_string(),
                "id_token".to_string(),
            ],
            subject_types_supported: vec!["public".to_string()],
            id_token_signing_alg_values_supported: vec!["RS256".to_string(), "ES256".to_string()],
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
struct OidcDiscovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    response_types_supported: Vec<String>,
    subject_types_supported: Vec<String>,
    id_token_signing_alg_values_supported: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
struct Jwk {
    kty: String,
    alg: String,
    use_: String,
    kid: String,
    n: Option<String>,   // RSA modulus
    e: Option<String>,   // RSA exponent
    x: Option<String>,   // EC x-coordinate
    y: Option<String>,   // EC y-coordinate
    d: Option<String>,   // Private key (optional, not exposed in public JWK)
    crv: Option<String>, // EC curve name
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Object)]
struct JwkSet {
    keys: Vec<Jwk>,
}

fn generate_jwk_set() -> JwkSet {
    let rsa_jwk = Jwk {
        kty: "RSA".to_string(),
        alg: "RS256".to_string(),
        use_: "sig".to_string(),
        kid: "rsa-key-id".to_string(),
        n: Some("base64url-modulus".to_string()), // Replace with actual base64url-encoded modulus
        e: Some("base64url-exponent".to_string()), // Replace with actual base64url-encoded exponent
        x: None,
        y: None,
        d: None,
        crv: None,
    };

    let ec_jwk = Jwk {
        kty: "EC".to_string(),
        alg: "ES256".to_string(),
        use_: "sig".to_string(),
        kid: "ec-key-id".to_string(),
        n: None,
        e: None,
        x: Some("base64url-x".to_string()), // Replace with actual base64url-encoded x-coordinate
        y: Some("base64url-y".to_string()), // Replace with actual base64url-encoded y-coordinate
        d: None,
        crv: Some("P-256".to_string()), // Curve name
    };

    JwkSet {
        keys: vec![rsa_jwk, ec_jwk],
    }
}

mod jwk {

    impl From<&jsonwebkey::JsonWebKey> for JsonWebKey {
        fn from(value: &jsonwebkey::JsonWebKey) -> Self {
            Self {
                key: value.key.clone().into(),
                key_use: value.key_use.map(|t| (&t).into()),
                // key_ops: value.key_ops,
                key_id: value.key_id.clone(),
                algorithm: value.algorithm.map(|t| (&t).into()),
                // x5: value.x5,
            }
        }
    }

    use base64ct::{Base64, Base64Url};
    use poem_openapi::{Enum, Object, Union};

    impl From<Box<jsonwebkey::Key>> for Key {
        fn from(value: Box<jsonwebkey::Key>) -> Self {
            match *value {
                jsonwebkey::Key::EC { curve } => Key::EC(curve.into()),
                jsonwebkey::Key::RSA { public, private } => todo!(),
                jsonwebkey::Key::Symmetric { key } => todo!(),
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Union)]
    #[oai(discriminator_name = "kty")]
    #[allow(clippy::upper_case_acronyms)]
    pub enum Key {
        /// An elliptic curve, as per [RFC 7518 §6.2](https://tools.ietf.org/html/rfc7518#section-6.2).
        EC(Curve),
        // /// An elliptic curve, as per [RFC 7518 §6.3](https://tools.ietf.org/html/rfc7518#section-6.3).
        // /// See also: [RFC 3447](https://tools.ietf.org/html/rfc3447).
        // RSA {
        //     #[oai(flatten)]
        //     public: RsaPublic,
        //     #[oai(flatten, default, skip_serializing_if = "Option::is_none")]
        //     private: Option<RsaPrivate>,
        // },
        // /// A symmetric key, as per [RFC 7518 §6.4](https://tools.ietf.org/html/rfc7518#section-6.4).
        // #[oai(rename = "oct")]
        // Symmetric {
        //     #[oai(rename = "k")]
        //     key: ByteVec,
        // },
    }

    impl From<jsonwebkey::Curve> for Curve {
        fn from(value: jsonwebkey::Curve) -> Self {
            use base64ct::Encoding;
            
            match value {
                jsonwebkey::Curve::P256 { d, x, y } => {
                    return Curve::P256(CurveP256 {
                        d: None,
                        x: Base64Url::encode_string(x.as_ref()),
                        y: Base64Url::encode_string(y.as_ref()),
                    })
                }
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq, Union)]
    #[oai(discriminator_name = "crv")]
    pub enum Curve {
        /// Parameters of the prime256v1 (P256) curve.
        // #[oai(rename = "P-256")]
        P256(CurveP256),
    }

    #[derive(Clone, Debug, PartialEq, Eq, Object)]
    #[oai(rename = "P-256")]
    pub struct CurveP256 {
        /// The private scalar.
        #[oai(skip_serializing_if = "Option::is_none")]
        d: Option<String>,
        /// The curve point x coordinate.
        x: String,
        /// The curve point y coordinate.
        y: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Object)]
    pub struct KeyOps {
        ops: Vec<String>,
    }

    impl KeyOps {
        fn is_empty(&self) -> bool {
            self.ops.is_empty()
        }
    }

    impl From<&jsonwebkey::KeyUse> for KeyUse {
        fn from(value: &jsonwebkey::KeyUse) -> Self {
            match value {
                jsonwebkey::KeyUse::Signing => KeyUse::Signing,
                jsonwebkey::KeyUse::Encryption => KeyUse::Encryption,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
    pub enum KeyUse {
        #[oai(rename = "sig")]
        Signing,
        #[oai(rename = "enc")]
        Encryption,
    }

    impl From<&jsonwebkey::Algorithm> for Algorithm {
        fn from(value: &jsonwebkey::Algorithm) -> Self {
            match value {
                jsonwebkey::Algorithm::HS256 => Algorithm::HS256,
                jsonwebkey::Algorithm::RS256 => Algorithm::RS256,
                jsonwebkey::Algorithm::ES256 => Algorithm::ES256,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Enum)]
    #[allow(clippy::upper_case_acronyms)]
    pub enum Algorithm {
        HS256,
        RS256,
        ES256,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Object)]
    pub struct JsonWebKey {
        #[oai(flatten)]
        pub key: Key,

        #[oai(default, rename = "use", skip_serializing_if = "Option::is_none")]
        pub key_use: Option<KeyUse>,

        // #[oai(default, skip_serializing_if = "KeyOps::is_empty")]
        // pub key_ops: KeyOps,
        #[oai(default, rename = "kid", skip_serializing_if = "Option::is_none")]
        pub key_id: Option<String>,

        #[oai(default, rename = "alg", skip_serializing_if = "Option::is_none")]
        pub algorithm: Option<Algorithm>,
        // #[oai(default, flatten, skip_serializing_if = "X509Params::is_empty")]
        // pub x5: X509Params,
    }

    // #[derive(Clone, Debug, Default, PartialEq, Eq, Object)]
    // pub struct X509Params {
    //     #[oai(default, rename = "x5u", skip_serializing_if = "Option::is_none")]
    //     url: Option<String>,

    //     #[oai(default, rename = "x5c", skip_serializing_if = "Option::is_none")]
    //     cert_chain: Option<Vec<String>>,

    //     #[oai(default, rename = "x5t", skip_serializing_if = "Option::is_none")]
    //     thumbprint: Option<String>,

    //     #[oai(default, rename = "x5t#S256", skip_serializing_if = "Option::is_none")]
    //     thumbprint_sha256: Option<String>,
    // }
}
