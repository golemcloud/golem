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

//! Account paths name accounts in the account tree.
//!
//! `/` is the root account, at depth zero. Every other path is `/` followed by one to
//! [`AccountPath::MAX_DEPTH`] segments separated by `/`, such as `/acme/customer-1`.
//!
//! A segment is 1 to [`AccountSegment::MAX_LEN`] characters from `a-z`, `0-9` and `-`. It
//! starts and ends with a letter or a digit and contains no `--`. Parsing never folds case,
//! trims, decodes or repairs: `/Acme`, `/acme%2Fx`, `//acme` and `/acme/` are invalid. The
//! longest path is therefore [`AccountPath::MAX_LEN`] characters.
//!
//! Because no segment is `-`, an environment reference such as `/acme/customer-1/-/shop/prod`
//! is never an account path, and the part before its first `/-/` is the account path.
//!
//! A path contains itself and its descendants, compared segment by segment, so `/acme/a`
//! does not contain `/acme/ab`.

use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountPathError {
    #[error("account path must start with '/'")]
    NotAbsolute,
    #[error("account path segment must not be empty")]
    EmptySegment,
    #[error("account path segment is {length} characters long, the maximum is {max}", max = AccountSegment::MAX_LEN)]
    SegmentTooLong { length: usize },
    #[error("account path segment contains {character:?}; only a-z, 0-9 and '-' are allowed")]
    InvalidCharacter { character: char },
    #[error("account path segment must start and end with a letter or a digit")]
    HyphenAtEdge,
    #[error("account path segment must not contain '--'")]
    DoubleHyphen,
    #[error("account path is deeper than {max} segments", max = AccountPath::MAX_DEPTH)]
    TooDeep,
}

/// One segment of an [`AccountPath`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountSegment(String);

impl AccountSegment {
    pub const MAX_LEN: usize = 64;

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AccountSegment {
    type Err = AccountPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err(AccountPathError::EmptySegment);
        }
        if let Some(character) = value
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
        {
            return Err(AccountPathError::InvalidCharacter { character });
        }
        if value.len() > Self::MAX_LEN {
            return Err(AccountPathError::SegmentTooLong {
                length: value.len(),
            });
        }
        if value.starts_with('-') || value.ends_with('-') {
            return Err(AccountPathError::HyphenAtEdge);
        }
        if value.contains("--") {
            return Err(AccountPathError::DoubleHyphen);
        }
        Ok(Self(value.to_string()))
    }
}

impl Display for AccountSegment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The path of an account in the account tree. The root account has no segments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AccountPath(Vec<AccountSegment>);

impl AccountPath {
    pub const MAX_DEPTH: usize = 8;
    /// Each segment adds at most its own length and one separator.
    pub const MAX_LEN: usize = Self::MAX_DEPTH * (AccountSegment::MAX_LEN + 1);

    pub fn root() -> Self {
        Self(Vec::new())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    pub fn segments(&self) -> &[AccountSegment] {
        &self.0
    }

    pub fn parent(&self) -> Option<AccountPath> {
        self.0
            .split_last()
            .map(|(_, ancestors)| Self(ancestors.to_vec()))
    }

    pub fn child(&self, segment: AccountSegment) -> Result<AccountPath, AccountPathError> {
        if self.depth() >= Self::MAX_DEPTH {
            return Err(AccountPathError::TooDeep);
        }
        let mut segments = self.0.clone();
        segments.push(segment);
        Ok(Self(segments))
    }

    /// Whether `other` is this account or one of its descendants.
    pub fn contains(&self, other: &AccountPath) -> bool {
        other.0.starts_with(&self.0)
    }
}

impl FromStr for AccountPath {
    type Err = AccountPathError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let rest = value
            .strip_prefix('/')
            .ok_or(AccountPathError::NotAbsolute)?;
        if rest.is_empty() {
            return Ok(Self::root());
        }
        let mut segments = Vec::new();
        for segment in rest.split('/') {
            if segments.len() == Self::MAX_DEPTH {
                return Err(AccountPathError::TooDeep);
            }
            segments.push(segment.parse()?);
        }
        Ok(Self(segments))
    }
}

impl TryFrom<&str> for AccountPath {
    type Error = AccountPathError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl Display for AccountPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return f.write_str("/");
        }
        for segment in &self.0 {
            write!(f, "/{segment}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{AccountPath, AccountPathError, AccountSegment};
    use crate::base_model::account::AccountEmail;
    use test_r::test;

    fn path(value: &str) -> AccountPath {
        value.parse().unwrap()
    }

    fn segments(count: usize, segment: &str) -> String {
        (0..count).map(|_| format!("/{segment}")).collect()
    }

    #[test]
    fn maximum_depth_and_segment_length_give_the_derived_maximum_length() {
        assert_eq!(AccountPath::MAX_LEN, 520);
        let longest = segments(8, &"a".repeat(64));
        assert_eq!(longest.len(), 520);

        let parsed = path(&longest);
        assert_eq!(parsed.depth(), 8);
        assert_eq!(parsed.to_string(), longest);
    }

    #[test]
    fn depth_and_segment_length_are_independent_limits() {
        assert_eq!(
            segments(9, "a").parse::<AccountPath>(),
            Err(AccountPathError::TooDeep)
        );
        assert_eq!(
            format!("/{}", "a".repeat(65)).parse::<AccountPath>(),
            Err(AccountPathError::SegmentTooLong { length: 65 })
        );
    }

    #[test]
    fn invalid_spellings_are_rejected_without_repair() {
        assert_eq!(
            AccountEmail::new("Owner@Example.com").as_str(),
            "owner@example.com"
        );
        assert_eq!(
            "/Acme".parse::<AccountPath>(),
            Err(AccountPathError::InvalidCharacter { character: 'A' })
        );

        for (value, error) in [
            ("/a--b", AccountPathError::DoubleHyphen),
            ("/-", AccountPathError::HyphenAtEdge),
            ("/-a", AccountPathError::HyphenAtEdge),
            ("/a-", AccountPathError::HyphenAtEdge),
            ("//a", AccountPathError::EmptySegment),
            ("/a/", AccountPathError::EmptySegment),
            ("/a//b", AccountPathError::EmptySegment),
            ("", AccountPathError::NotAbsolute),
            ("acme", AccountPathError::NotAbsolute),
            (
                "/acme%2Fx",
                AccountPathError::InvalidCharacter { character: '%' },
            ),
            (
                "/acme/%2f",
                AccountPathError::InvalidCharacter { character: '%' },
            ),
            (
                "/a/./b",
                AccountPathError::InvalidCharacter { character: '.' },
            ),
            (
                "/acmé",
                AccountPathError::InvalidCharacter { character: 'é' },
            ),
            (
                "/a b",
                AccountPathError::InvalidCharacter { character: ' ' },
            ),
            (
                "/a\\b",
                AccountPathError::InvalidCharacter { character: '\\' },
            ),
        ] {
            assert_eq!(value.parse::<AccountPath>(), Err(error), "{value:?}");
        }

        for value in ["/0", "/a-b", "/a-b-c", "/acme/customer-1"] {
            assert_eq!(path(value).to_string(), value);
        }
    }

    #[test]
    fn root_has_depth_zero() {
        let root = path("/");
        assert_eq!(root, AccountPath::root());
        assert!(root.is_root());
        assert_eq!(root.depth(), 0);
        assert_eq!(root.parent(), None);
        assert_eq!(root.to_string(), "/");

        let acme = root.child("acme".parse().unwrap()).unwrap();
        assert_eq!(acme, path("/acme"));
        assert_eq!(acme.parent(), Some(root));

        let deepest = path(&segments(8, "a"));
        assert_eq!(
            deepest.child("b".parse().unwrap()),
            Err(AccountPathError::TooDeep)
        );
    }

    #[test]
    fn containment_respects_segment_boundaries() {
        assert!(!path("/acme/a").contains(&path("/acme/ab")));
        assert!(path("/acme/a").contains(&path("/acme/a/b")));
        assert!(path("/acme/a").contains(&path("/acme/a")));
        assert!(path("/").contains(&path("/acme")));
        assert!(!path("/acme").contains(&path("/")));
        assert!(!path("/acme/a/b").contains(&path("/acme/a")));
    }

    #[test]
    fn reference_fixtures_name_accounts() {
        // CLI `--account`, manifest `environments.prod.account`, card recipient, package
        // publisher and the reserved publisher account.
        for value in ["/acme/customer-1", "/acme", "/golem", "/"] {
            assert_eq!(path(value).to_string(), value);
        }

        // The account part of the subtree owner scope `/acme/customer-1/**`.
        let owner_scope = "/acme/customer-1/**";
        let account = owner_scope.strip_suffix("/**").unwrap();
        assert!(path(account).contains(&path("/acme/customer-1/team")));
        assert!(owner_scope.parse::<AccountPath>().is_err());
    }

    #[test]
    fn environment_references_are_never_account_paths() {
        assert_eq!(
            "-".parse::<AccountSegment>(),
            Err(AccountPathError::HyphenAtEdge)
        );

        for (reference, account) in [
            ("/acme/customer-1/-/shop/prod", "/acme/customer-1"),
            ("/acme/customer-1/-/shop/*", "/acme/customer-1"),
            ("/acme/customer-1/-/*/*", "/acme/customer-1"),
            ("/-/shop/prod", "/"),
        ] {
            assert!(reference.parse::<AccountPath>().is_err(), "{reference}");

            let (prefix, _) = reference.split_once("/-/").unwrap();
            let prefix = if prefix.is_empty() { "/" } else { prefix };
            assert_eq!(path(prefix), path(account), "{reference}");
        }
    }
}
