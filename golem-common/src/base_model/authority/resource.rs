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

use crate::base_model::auth::TokenId;
use crate::base_model::caller::Pair;
use crate::base_model::card::CardId;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// The concrete thing that one part of a request asks for: the resource side of a grant, with a
/// value instead of a pattern. The catalog states which [`ResourceKind`] each part takes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Resource {
    /// The grant's resource is empty, as in `agent(<agent-id>) : exec :`.
    None,
    /// Every resource of the verb, as the tool-wide `invoke : *` that a static `File` read needs.
    All,
    /// An agent method.
    Method(String),
    /// A tool command.
    Command(String),
    /// A path: a file or directory, or a secret path.
    Path(String),
    /// The destination of an outgoing connection.
    Destination(Destination),
    /// A named resource: a quota resource, a host or domain, a protocol, a spend mode.
    Name(String),
    /// A key-value store and a key, or a blob container and an object.
    Store {
        store: String,
        key: String,
    },
    /// A database, and optionally a schema and a table in it.
    Database {
        database: String,
        schema: Option<String>,
        table: Option<String>,
    },
    Card(CardId),
    /// The pair that a card is installed into.
    Pair(Pair),
    /// An account API token.
    ApiToken(TokenId),
}

/// The kind of a [`Resource`], without its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    None,
    All,
    Method,
    Command,
    Path,
    Destination,
    Name,
    Store,
    Database,
    Card,
    Pair,
    ApiToken,
}

impl Resource {
    pub fn kind(&self) -> ResourceKind {
        match self {
            Resource::None => ResourceKind::None,
            Resource::All => ResourceKind::All,
            Resource::Method(_) => ResourceKind::Method,
            Resource::Command(_) => ResourceKind::Command,
            Resource::Path(_) => ResourceKind::Path,
            Resource::Destination(_) => ResourceKind::Destination,
            Resource::Name(_) => ResourceKind::Name,
            Resource::Store { .. } => ResourceKind::Store,
            Resource::Database { .. } => ResourceKind::Database,
            Resource::Card(_) => ResourceKind::Card,
            Resource::Pair(_) => ResourceKind::Pair,
            Resource::ApiToken(_) => ResourceKind::ApiToken,
        }
    }
}

/// The destination of an outgoing connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "form"
)]
pub enum Destination {
    /// A host and an optional port, before resolution, as an HTTP client names them.
    HostPort { host: String, port: Option<u16> },
    /// A socket, on the address that the executor resolved. A wildcard host reaches only a public
    /// address that is not an address of the executor's own host, and a grant that names one of
    /// the linked host names reaches the address.
    Socket {
        address: IpAddress,
        port: u16,
        /// The address is an address of the executor's own host.
        this_host: bool,
        /// The normalized host names that resolved to the address.
        hostnames: Vec<String>,
    },
}

/// An IP address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IpAddress(pub IpAddr);

#[cfg(feature = "full")]
impl desert_rust::BinarySerializer for IpAddress {
    fn serialize<Output: desert_rust::BinaryOutput>(
        &self,
        context: &mut desert_rust::SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        desert_rust::BinarySerializer::serialize(&self.0.to_string(), context)
    }
}

#[cfg(feature = "full")]
impl desert_rust::BinaryDeserializer for IpAddress {
    fn deserialize(
        context: &mut desert_rust::DeserializationContext<'_>,
    ) -> desert_rust::Result<Self> {
        let text: String = desert_rust::BinaryDeserializer::deserialize(context)?;
        text.parse::<IpAddr>()
            .map(IpAddress)
            .map_err(|error| desert_rust::Error::DeserializationFailure(error.to_string()))
    }
}
