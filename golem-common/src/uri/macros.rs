// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[macro_export]
macro_rules! urn_from_into {
    ($name:ident) => {
        impl TryFrom<&GolemUrn> for $name {
            type Error = GolemUrnTransformError;

            fn try_from(value: &GolemUrn) -> Result<Self, Self::Error> {
                try_from_golem_urn(value)
            }
        }

        impl TryFrom<GolemUrn> for $name {
            type Error = GolemUrnTransformError;

            fn try_from(value: GolemUrn) -> Result<Self, Self::Error> {
                try_from_golem_urn(&value)
            }
        }

        impl FromStr for $name {
            type Err = GolemUrnTransformError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let urn = GolemUrn::from_str(s)
                    .map_err(|err| GolemUrnTransformError::UrnParseError { err })?;

                urn.try_into()
            }
        }

        impl From<&$name> for GolemUrn {
            fn from(value: &$name) -> Self {
                GolemUrn {
                    resource_type: $name::resource_type().to_string(),
                    resource_name: value.to_name(),
                }
            }
        }

        impl From<$name> for GolemUrn {
            fn from(value: $name) -> Self {
                GolemUrn::from(&value)
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", GolemUrn::from(self))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                FromStr::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

#[macro_export]
macro_rules! url_from_into {
    ($name:ident) => {
        impl TryFrom<&GolemUrl> for $name {
            type Error = GolemUrlTransformError;

            fn try_from(value: &GolemUrl) -> Result<Self, Self::Error> {
                try_from_golem_url(value)
            }
        }

        impl TryFrom<GolemUrl> for $name {
            type Error = GolemUrlTransformError;

            fn try_from(value: GolemUrl) -> Result<Self, Self::Error> {
                try_from_golem_url(&value)
            }
        }

        impl FromStr for $name {
            type Err = GolemUrlTransformError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let url = GolemUrl::from_str(s)
                    .map_err(|err| GolemUrlTransformError::UrlParseError { err })?;

                url.try_into()
            }
        }

        impl From<&$name> for GolemUrl {
            fn from(value: &$name) -> Self {
                let (path, query) = value.to_parts();

                GolemUrl {
                    resource_type: $name::resource_type().to_string(),
                    path,
                    query,
                }
            }
        }

        impl From<$name> for GolemUrl {
            fn from(value: $name) -> Self {
                GolemUrl::from(&value)
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", GolemUrl::from(self))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                FromStr::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

#[macro_export]
macro_rules! uri_from_into {
    ($name:ident) => {
        impl TryFrom<&GolemUri> for $name {
            type Error = GolemUriTransformError;

            fn try_from(value: &GolemUri) -> Result<Self, Self::Error> {
                match value {
                    GolemUri::URL(url) => Ok($name::URL(
                        url.try_into().map_err(GolemUriTransformError::URL)?,
                    )),
                    GolemUri::URN(urn) => Ok($name::URN(
                        urn.try_into().map_err(GolemUriTransformError::URN)?,
                    )),
                }
            }
        }

        impl TryFrom<GolemUri> for $name {
            type Error = GolemUriTransformError;

            fn try_from(value: GolemUri) -> Result<Self, Self::Error> {
                (&value).try_into()
            }
        }

        impl FromStr for $name {
            type Err = GolemUriTransformError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let uri = GolemUri::from_str(s).map_err(|err| match err {
                    GolemUriParseError::URN(err) => {
                        GolemUriTransformError::URN(GolemUrnTransformError::UrnParseError { err })
                    }
                    GolemUriParseError::URL(err) => {
                        GolemUriTransformError::URL(GolemUrlTransformError::UrlParseError { err })
                    }
                })?;

                uri.try_into()
            }
        }

        impl From<&$name> for GolemUri {
            fn from(value: &$name) -> Self {
                match value {
                    $name::URN(urn) => GolemUri::URN(urn.into()),
                    $name::URL(url) => GolemUri::URL(url.into()),
                }
            }
        }

        impl From<$name> for GolemUri {
            fn from(value: $name) -> Self {
                GolemUri::from(&value)
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", GolemUri::from(self))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                FromStr::from_str(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}
