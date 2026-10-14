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

use crate::base_model::environment::EnvironmentId;
use crate::{declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;

newtype_uuid!(DomainRegistrationId);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct Domain(pub String);
}

declare_structs! {
    pub struct DomainRegistrationCreation {
        pub domain: Domain
    }

    pub struct DomainRegistration {
        pub id: DomainRegistrationId,
        pub environment_id: EnvironmentId,
        pub domain: Domain
    }
}

impl Domain {
    /// The host of the domain's authority: lowercased, without a port and without a trailing dot.
    /// IPv6 literals are in their canonical form.
    pub fn host(&self) -> String {
        let authority = self.0.trim();
        let host = if authority.starts_with('[') {
            authority
                .find(']')
                .map_or(authority, |close| &authority[..=close])
        } else {
            match authority.rsplit_once(':') {
                Some((host, port))
                    if !host.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    host
                }
                _ => authority,
            }
        };
        canonical_host(host)
    }

    /// Parses a configured host name, such as a reserved domain or the management host, and
    /// returns it the way [`Domain::host`] does. The value is a DNS name, an IPv4 address or a
    /// bracketed IPv6 address, with an optional port; schemes, paths and user info are rejected.
    pub fn parse_host(value: &str) -> Result<String, String> {
        let (host, _) = split_authority(value).ok_or_else(|| format!("Invalid host: {value:?}"))?;
        Ok(Domain(host.to_string()).host())
    }

    /// The canonical spelling of the domain: the host lowercased and without a trailing dot, an
    /// IPv6 literal in its compressed form, and the port, if any, without leading zeros. Requests
    /// are routed by this spelling, so a domain is only registered in it.
    pub fn canonical(&self) -> Result<Domain, String> {
        let (host, port) =
            split_authority(&self.0).ok_or_else(|| format!("Invalid domain: {:?}", self.0))?;
        let host = canonical_host(host);
        match port.strip_prefix(':') {
            Some(port) => {
                let port: u16 = port
                    .parse()
                    .map_err(|_| format!("Invalid domain: {:?}", self.0))?;
                Ok(Domain(format!("{host}:{port}")))
            }
            None => Ok(Domain(host)),
        }
    }
}

/// The canonical spelling of a host: a bracketed IPv6 literal rendered from its address
/// (compressed, lowercase), any other host lowercased and without a trailing dot.
fn canonical_host(host: &str) -> String {
    if let Some(Ok(address)) = host
        .strip_prefix('[')
        .and_then(|literal| literal.strip_suffix(']'))
        .map(str::parse::<std::net::Ipv6Addr>)
    {
        return format!("[{address}]");
    }
    host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
}

/// Splits a valid authority into its host and its port suffix (empty or `:<port>`).
fn split_authority(value: &str) -> Option<(&str, &str)> {
    let (host, port) = match value.strip_prefix('[') {
        Some(rest) => {
            let (literal, rest) = rest.split_once(']')?;
            literal.parse::<std::net::Ipv6Addr>().ok()?;
            (&value[..literal.len() + 2], rest)
        }
        None => match value.split_once(':') {
            Some((host, _)) => (host, &value[host.len()..]),
            None => (value, ""),
        },
    };
    if !port.is_empty()
        && port.strip_prefix(':').is_none_or(|port| {
            !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err()
        })
    {
        return None;
    }
    if !host.starts_with('[') {
        let name = host.strip_suffix('.').unwrap_or(host);
        if name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && name.parse::<std::net::Ipv4Addr>().is_err()
        {
            return None;
        }
        let valid = !name.is_empty()
            && name.len() <= 253
            && name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
        if !valid {
            return None;
        }
    }
    Some((host, port))
}

#[cfg(test)]
mod tests {
    use super::Domain;
    use test_r::test;

    #[test]
    fn host_ignores_case_port_and_trailing_dot() {
        for authority in [
            "api.golem.example.com",
            "API.Golem.Example.com",
            "api.golem.example.com:443",
            "api.golem.example.com.",
            "api.golem.example.com.:9881",
        ] {
            assert_eq!(
                Domain(authority.to_string()).host(),
                "api.golem.example.com",
                "{authority}"
            );
        }
        assert_eq!(Domain("[::1]:9881".to_string()).host(), "[::1]");
        assert_eq!(Domain("[0:0::1]".to_string()).host(), "[::1]");
        assert_eq!(Domain("[".to_string()).host(), "[");
        assert_eq!(Domain("localhost".to_string()).host(), "localhost");
    }

    #[test]
    fn parse_host_accepts_hosts_with_optional_ports() {
        for (value, host) in [
            ("api.golem.example.com", "api.golem.example.com"),
            ("API.golem.example.com:443", "api.golem.example.com"),
            ("api.golem.example.com.", "api.golem.example.com"),
            ("localhost:9881", "localhost"),
            ("127.0.0.1", "127.0.0.1"),
            ("[::1]:9881", "[::1]"),
        ] {
            assert_eq!(Domain::parse_host(value).unwrap(), host, "{value}");
        }
    }

    #[test]
    fn canonical_lowercases_the_host_drops_a_trailing_dot_and_keeps_the_port() {
        for (value, canonical) in [
            ("api.example.com", "api.example.com"),
            ("API.Example.com", "api.example.com"),
            ("api.example.com.", "api.example.com"),
            ("Api.Example.Com.:9006", "api.example.com:9006"),
            ("api.example.com:443", "api.example.com:443"),
            ("localhost:9006", "localhost:9006"),
            ("api.example.com:09006", "api.example.com:9006"),
            ("[::1]:9006", "[::1]:9006"),
            ("[0:0::1]:9006", "[::1]:9006"),
            ("[0:0:0:0:0:0:0:1]:09006", "[::1]:9006"),
            ("[2001:DB8::1]", "[2001:db8::1]"),
            ("[2001:db8:0:0:1:0:0:1]", "[2001:db8::1:0:0:1]"),
            ("[2001:db8:0:1:1:1:1:1]", "[2001:db8:0:1:1:1:1:1]"),
            ("[::ffff:10.0.0.5]", "[::ffff:10.0.0.5]"),
            ("[::FFFF:A00:5]", "[::ffff:10.0.0.5]"),
            ("[::1.2.3.4]", "[::102:304]"),
            ("1.2.3.4.", "1.2.3.4"),
        ] {
            assert_eq!(
                Domain(value.to_string()).canonical().unwrap(),
                Domain(canonical.to_string()),
                "{value}"
            );
        }
        for value in [
            "",
            "https://api.example.com",
            "api..example.com",
            "api.example.com:",
            "[fe80::1%25eth0]",
            "[fe80::1%eth0]",
            "[v1.a]",
        ] {
            assert!(Domain(value.to_string()).canonical().is_err(), "{value}");
        }
        for value in ["API.Example.com.:09006", "[0:0::1]:9006", "[::FFFF:A00:5]"] {
            let once = Domain(value.to_string()).canonical().unwrap();
            assert_eq!(once.canonical().unwrap(), once, "{value}");
        }
    }

    #[test]
    fn parse_host_rejects_values_that_are_not_hosts() {
        for value in [
            "",
            ":9881",
            "https://api.golem.example.com",
            "api.golem.example.com/",
            "user@api.golem.example.com",
            "api golem.example.com",
            "api..golem.example.com",
            "api.golem.example.com:",
            "api.golem.example.com:99999",
            "api.golem.example.com:+443",
            "1.2.3",
            "999.1.1.1",
            "-api.golem.example.com",
            "[",
            "[not-ipv6]",
        ] {
            assert!(Domain::parse_host(value).is_err(), "{value}");
        }
    }
}
