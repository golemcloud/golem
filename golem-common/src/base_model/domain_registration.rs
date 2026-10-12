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
            let literal = authority
                .find(']')
                .map_or(authority, |close| &authority[..=close]);
            if let Some(Ok(address)) = literal
                .strip_prefix('[')
                .and_then(|literal| literal.strip_suffix(']'))
                .map(str::parse::<std::net::Ipv6Addr>)
            {
                return format!("[{address}]");
            }
            literal
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
        host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase()
    }

    /// Parses a configured host name, such as a reserved domain or the management host, and
    /// returns it the way [`Domain::host`] does. The value is a DNS name, an IPv4 address or a
    /// bracketed IPv6 address, with an optional port; schemes, paths and user info are rejected.
    pub fn parse_host(value: &str) -> Result<String, String> {
        let invalid = || format!("Invalid host: {value:?}");
        let (host, port) = match value.strip_prefix('[') {
            Some(rest) => {
                let (literal, rest) = rest.split_once(']').ok_or_else(invalid)?;
                literal
                    .parse::<std::net::Ipv6Addr>()
                    .map_err(|_| invalid())?;
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
            return Err(invalid());
        }
        if !host.starts_with('[') {
            let name = host.strip_suffix('.').unwrap_or(host);
            if name.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                && name.parse::<std::net::Ipv4Addr>().is_err()
            {
                return Err(invalid());
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
                return Err(invalid());
            }
        }
        Ok(Domain(host.to_string()).host())
    }
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
