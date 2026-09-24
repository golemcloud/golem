// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

use super::owner::EmptyOwnerPattern;
use super::{
    ClassPermissionTarget, NetworkResourcePattern, NetworkVerb, PermissionTarget, PortPattern,
    ResourcePattern,
};
use std::net::IpAddr;
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetworkTargetError {
    #[error("invalid network address: {0}")]
    InvalidNetworkAddress(String),
    #[error("invalid outbound URL: {0}")]
    InvalidUrl(String),
    #[error("invalid network resource: {0}")]
    InvalidResource(String),
}

pub fn network_target(
    host: &str,
    port: Option<u16>,
) -> Result<PermissionTarget, NetworkTargetError> {
    let host = normalize_host(host)?;
    Ok(PermissionTarget::Network(ClassPermissionTarget {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        resource: NetworkResourcePattern::host_port(
            host,
            port.map(PortPattern::single)
                .unwrap_or_else(PortPattern::any),
        ),
    }))
}

fn normalize_host(host: &str) -> Result<String, NetworkTargetError> {
    let host = host.trim().trim_end_matches('.');
    if host.is_empty()
        || host.contains(':')
        || host.contains('*')
        || host.chars().any(char::is_whitespace)
    {
        return Err(NetworkTargetError::InvalidNetworkAddress(host.to_string()));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => Ok(v4.to_string()),
            IpAddr::V6(_) => Err(NetworkTargetError::InvalidNetworkAddress(host.to_string())),
        };
    }
    if host.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(NetworkTargetError::InvalidNetworkAddress(host.to_string()));
    }
    let normalized = host.to_ascii_lowercase();
    NetworkResourcePattern::parse_resource(&normalized)
        .map_err(|_| NetworkTargetError::InvalidResource(normalized.clone()))?;
    Ok(normalized)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedHttpTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub permission: PermissionTarget,
}

pub fn http_target(uri: &str) -> Result<NormalizedHttpTarget, NetworkTargetError> {
    uri_target(uri, &["http", "https"])
}

pub fn websocket_target(uri: &str) -> Result<NormalizedHttpTarget, NetworkTargetError> {
    uri_target(uri, &["ws", "wss"])
}

fn uri_target(uri: &str, schemes: &[&str]) -> Result<NormalizedHttpTarget, NetworkTargetError> {
    let url = Url::parse(uri).map_err(|_| NetworkTargetError::InvalidUrl(uri.to_string()))?;
    if !schemes.contains(&url.scheme()) || !url.username().is_empty() || url.password().is_some() {
        return Err(NetworkTargetError::InvalidUrl(uri.to_string()));
    }
    let host = normalize_host(
        url.host_str()
            .ok_or_else(|| NetworkTargetError::InvalidUrl(uri.to_string()))?,
    )?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| NetworkTargetError::InvalidUrl(uri.to_string()))?;
    Ok(NormalizedHttpTarget {
        scheme: url.scheme().to_ascii_lowercase(),
        host: host.clone(),
        port,
        path: url.path().to_string(),
        permission: network_target(&host, Some(port))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn network_and_http_are_canonical() {
        assert!(network_target("127.000.000.001", Some(80)).is_err());
        let h = http_target("HTTPS://Example.COM.:443/a/../b?q=1").unwrap();
        assert_eq!(
            (h.scheme.as_str(), h.host.as_str(), h.port, h.path.as_str()),
            ("https", "example.com", 443, "/b")
        );
        assert_eq!(
            h.permission,
            network_target("example.com", Some(443)).unwrap()
        );
        let websocket = websocket_target("WSS://Example.COM./socket").unwrap();
        assert_eq!(
            (websocket.host.as_str(), websocket.port),
            ("example.com", 443)
        );
    }

    #[test]
    fn parsed_network_grants_use_the_same_hostname_normalization_as_runtime_targets() {
        let grant = NetworkResourcePattern::parse_resource("Example.COM:443").unwrap();
        let target = http_target("https://example.com/").unwrap().permission;
        let PermissionTarget::Network(target) = target else {
            panic!("HTTP must produce a network target");
        };
        assert!(grant.subsumes(&target.resource));
    }

    #[test]
    fn parsed_network_grants_normalize_trailing_dot_like_runtime_targets() {
        let grant = NetworkResourcePattern::parse_resource("Example.COM.:443").unwrap();
        let target = http_target("https://example.com./").unwrap().permission;
        let PermissionTarget::Network(target) = target else {
            panic!("HTTP must produce a network target");
        };
        assert!(grant.subsumes(&target.resource));
    }

    #[test]
    fn malformed_network_and_urls_fail_closed() {
        assert!(network_target("::1", Some(80)).is_err());
        assert!(network_target("*.example.com", None).is_err());
        assert!(http_target("ftp://example.com/a").is_err());
        assert!(http_target("https://u:p@example.com").is_err());
    }
}
