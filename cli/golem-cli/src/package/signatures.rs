// Copyright 2024-2026 Golem Cloud
// SPDX-License-Identifier: LicenseRef-Golem-Source-License-1.1

use super::remote::{OciRegistry, response_bytes};
use anyhow::{Context, ensure};
use futures_util::StreamExt;
use reqwest::{StatusCode, header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sigstore_crypto::{VerificationKey, validate_raw_public_key};
use sigstore_types::DerPublicKey;
use sigstore_types::bundle::{Bundle, MediaType, SignatureContent};
use sigstore_types::intoto::Statement;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const INDEX: &str = "application/vnd.oci.image.index.v1+json";
const BUNDLE: &str = "application/vnd.dev.sigstore.bundle.v0.3+json";
const PREDICATE: &str = "https://sigstore.dev/cosign/sign/v1";
const MAX_METADATA: u64 = 256 * 1024;
const MAX_BUNDLE: u64 = 512 * 1024;
const MAX_SIGNATURES: usize = 16;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    digest: String,
    size: u64,
    media_type: String,
    artifact_type: Option<String>,
    #[serde(default)]
    annotations: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Index {
    manifests: Vec<Descriptor>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Referrer {
    artifact_type: String,
    subject: Descriptor,
    layers: Vec<Descriptor>,
}

pub(super) async fn report(registry: &OciRegistry, digest: &str, trust_store: Option<&Path>) {
    use crate::log::eprint_line;

    let keys = match load_keys(trust_store, include_bytes!("trust-store.pem")) {
        Ok(keys) => keys,
        Err(cause) => {
            eprint_line(format!("Package signer unverifiable ({cause})"));
            return;
        }
    };
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        let (descriptors, more) = discover(registry, digest).await?;
        let mut signatures = descriptors
            .into_iter()
            .filter(|desc| {
                desc.artifact_type.as_deref() == Some(BUNDLE)
                    && desc
                        .annotations
                        .get("dev.sigstore.bundle.predicateType")
                        .is_none_or(|predicate| predicate == PREDICATE)
            })
            .peekable();
        if signatures.peek().is_none() {
            if more {
                eprint_line("Package signature discovery incomplete (additional referrers)");
            } else {
                eprint_line("Package has no Cosign signature");
            }
            return Ok(());
        }
        futures_util::stream::iter(signatures.by_ref().take(MAX_SIGNATURES))
            .for_each_concurrent(4, |desc| {
                let keys = &keys;
                async move {
                    let result = match load_bundle(registry, digest, &desc).await {
                        Ok(bundle) => verify_bundle(&bundle, digest, keys),
                        Err(_) => Err("invalid or unavailable Cosign referrer"),
                    };
                    match result {
                        Ok(signer) => eprint_line(format!("Package signer {signer:?} verified")),
                        Err(cause) => eprint_line(format!("Package signer unverifiable ({cause})")),
                    }
                }
            })
            .await;
        if more || signatures.next().is_some() {
            eprint_line("Package signature discovery incomplete (additional referrers)");
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => eprint_line(
            "Package signature discovery failed (invalid or unavailable registry response)",
        ),
        Err(_) => eprint_line("Package signature discovery incomplete (deadline exceeded)"),
    }
}

async fn discover(registry: &OciRegistry, digest: &str) -> anyhow::Result<(Vec<Descriptor>, bool)> {
    let response = registry.get(&format!("referrers/{digest}"), INDEX).await?;
    let response = if response.status() == StatusCode::NOT_FOUND {
        // OCI 1.1 fallback index, not Cosign's legacy .sig tag.
        let tag = digest.replace(':', "-");
        let response = registry.get(&format!("manifests/{tag}"), INDEX).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok((Vec::new(), false));
        }
        response
    } else {
        response
    };
    let more = response.headers().contains_key(header::LINK);
    let index: Index = serde_json::from_slice(&response_bytes(response, MAX_METADATA).await?)?;
    Ok((index.manifests, more))
}

async fn load_bundle(
    registry: &OciRegistry,
    digest: &str,
    desc: &Descriptor,
) -> anyhow::Result<Bundle> {
    ensure!(
        desc.media_type == super::MANIFEST,
        "Not an OCI image manifest"
    );
    let manifest = fetch_descriptor(registry, "manifests", desc, MAX_METADATA).await?;
    let referrer: Referrer = serde_json::from_slice(&manifest)?;
    ensure!(
        referrer.artifact_type == BUNDLE,
        "Not a Sigstore bundle referrer"
    );
    ensure!(referrer.subject.digest == digest, "Wrong referrer subject");
    let [layer] = referrer.layers.as_slice() else {
        anyhow::bail!("Expected one Sigstore bundle layer");
    };
    ensure!(
        layer.media_type == BUNDLE,
        "Wrong Sigstore bundle layer type"
    );
    let bytes = fetch_descriptor(registry, "blobs", layer, MAX_BUNDLE).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn fetch_descriptor(
    registry: &OciRegistry,
    kind: &str,
    desc: &Descriptor,
    limit: u64,
) -> anyhow::Result<Vec<u8>> {
    let hex = desc
        .digest
        .strip_prefix("sha256:")
        .context("Expected SHA-256")?;
    ensure!(
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid digest"
    );
    ensure!(desc.size <= limit, "Signature metadata exceeds size limit");
    let response = registry
        .get(&format!("{kind}/{}", desc.digest), &desc.media_type)
        .await?;
    let bytes = response_bytes(response, desc.size).await?;
    ensure!(
        bytes.len() as u64 == desc.size && format!("{:x}", Sha256::digest(&bytes)) == hex,
        "Descriptor mismatch"
    );
    Ok(bytes)
}

fn load_keys(
    path: Option<&Path>,
    bundled: &[u8],
) -> Result<Vec<(String, VerificationKey)>, &'static str> {
    let override_bytes;
    let bytes = if let Some(path) = path {
        override_bytes = super::read_regular(path, MAX_METADATA)
            .map_err(|_| "cannot read signer trust store")?;
        override_bytes.as_slice()
    } else {
        bundled
    };
    const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
    const END: &str = "-----END PUBLIC KEY-----";
    const INVALID: &str = "invalid signer trust store PEM (expected PUBLIC KEY blocks)";
    let mut rest = std::str::from_utf8(bytes).map_err(|_| INVALID)?.trim();
    let mut keys = Vec::new();
    // Sigstore decodes one PEM block at a time. Do not skip text or other block types.
    while !rest.is_empty() {
        if keys.len() == 32 {
            return Err("signer trust store has more than 32 keys");
        }
        let (body, _) = rest
            .strip_prefix(BEGIN)
            .and_then(|s| s.split_once(END))
            .ok_or(INVALID)?;
        if body.contains("-----") {
            return Err(INVALID);
        }
        let end = BEGIN.len() + body.len() + END.len();
        let der = DerPublicKey::from_pem(&rest[..end]).map_err(|_| INVALID)?;
        let key = VerificationKey::from_spki(&der)
            .map_err(|_| "invalid or unsupported signer public key")?;
        validate_raw_public_key(key.scheme(), key.as_bytes())
            .map_err(|_| "invalid signer public key")?;
        // Fingerprint the DER SPKI so the label is independent of PEM formatting.
        let signer = format!("sha256:{:x}", Sha256::digest(der.as_bytes()));
        keys.push((signer, key));
        rest = rest[end..].trim_start();
    }
    if keys.is_empty() {
        return Err("signer trust store has no usable keys");
    }
    Ok(keys)
}

fn verify_bundle<'a>(
    bundle: &Bundle,
    digest: &str,
    keys: &'a [(String, VerificationKey)],
) -> Result<&'a str, &'static str> {
    let SignatureContent::DsseEnvelope(envelope) = &bundle.content else {
        return Err("unsupported Cosign signature format");
    };
    if bundle.media_type != MediaType::Bundle0_3
        || envelope.payload_type != "application/vnd.in-toto+json"
    {
        return Err("unsupported Cosign signature format");
    }
    // Neither the key hint nor any certificate in the bundle establishes trust.
    let pae = envelope.pae();
    let (signer, _) = keys
        .iter()
        .find(|(_, key)| key.verify(&pae, &envelope.signature.sig).is_ok())
        .ok_or("signature does not verify with a trusted signing key")?;
    let statement: Statement = serde_json::from_slice(envelope.payload.as_bytes())
        .map_err(|_| "invalid signed Cosign statement")?;
    if statement.type_ != "https://in-toto.io/Statement/v1" || statement.predicate_type != PREDICATE
    {
        return Err("wrong signed Cosign statement type");
    }
    if let [subject] = statement.subject.as_slice()
        && subject
            .digest
            .sha256
            .is_some_and(|sha| format!("sha256:{sha}") == digest)
    {
        return Ok(signer);
    }
    Err("signature does not bind the installed OCI manifest digest")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn bundled_trust_store_contains_only_the_golem_key() {
        let keys = load_keys(None, include_bytes!("trust-store.pem")).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(
            keys[0].0,
            "sha256:7bd0e4be92090c9e4a5c74029a7784c0a56b2237042171fa9cd37208cb10aaf5"
        );
    }

    #[test]
    fn pem_store_rejects_non_public_key_blocks_and_truncated_keys() {
        let key = include_str!("trust-store.pem");
        for invalid in [
            key.replace("PUBLIC KEY", "CERTIFICATE"),
            format!("-----BEGIN PUBLIC KEY-----\n{key}"),
            format!("{key}-----BEGIN PUBLIC KEY-----\n"),
            r#"{"keys":[]}"#.to_owned(),
        ] {
            assert_eq!(
                load_keys(None, invalid.as_bytes()).err(),
                Some("invalid signer trust store PEM (expected PUBLIC KEY blocks)")
            );
        }
    }

    #[test]
    fn cosign_bundle_verifies_only_the_pinned_key_and_installed_digest() {
        #[derive(Deserialize)]
        struct Fixture {
            digest: String,
            signer: String,
            bundle: Bundle,
        }

        // Cosign signed the package with a disposable key, never trusted by default.
        let Fixture {
            digest,
            signer,
            mut bundle,
        } = serde_json::from_str(include_str!(
            "../../test-data/cosign-pinned-key-bundle.json"
        ))
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signers.pem");
        let other = include_str!("trust-store.pem");
        let bundled = format!("{other}\n{signer}").replace('\n', "\r\n");
        std::fs::write(&path, &bundled).unwrap();
        let keys = load_keys(Some(&path), other.as_bytes()).unwrap();
        assert_eq!(
            verify_bundle(&bundle, &digest, &keys),
            Ok("sha256:fb0d2942cab2a63b4479a1e3e378d9eff7408bc57876fb07fe1eb723c8e47b82")
        );
        std::fs::write(&path, other).unwrap();
        assert_eq!(
            verify_bundle(
                &bundle,
                &digest,
                &load_keys(Some(&path), bundled.as_bytes()).unwrap()
            ),
            Err("signature does not verify with a trusted signing key")
        );
        assert_eq!(
            verify_bundle(&bundle, &format!("sha256:{}", "b".repeat(64)), &keys),
            Err("signature does not bind the installed OCI manifest digest")
        );
        let SignatureContent::DsseEnvelope(envelope) = &mut bundle.content else {
            panic!("Expected a Cosign DSSE envelope")
        };
        let mut signature = envelope.signature.sig.as_bytes().to_vec();
        *signature.last_mut().unwrap() ^= 1;
        envelope.signature.sig = sigstore_types::SignatureBytes::new(signature);
        assert_eq!(
            verify_bundle(&bundle, &digest, &keys),
            Err("signature does not verify with a trusted signing key")
        );
    }
}
