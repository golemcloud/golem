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

//! Method-passable handle for the rich [`SchemaValue::Secret`] capability.
//!
//! A secret value carries only a plaintext-free identity snapshot; the secret
//! material itself is never embedded. This legacy wrapper lets host-side callers
//! bridge string secret identifiers into [`SchemaValue::Secret`] while the
//! capability is redacted at CLI/tracing surfaces.

use super::{
    FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, secret_from_value, secret_to_value,
};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{SchemaType, SecretSpec};
use crate::schema::schema_value::SchemaValue;

/// A host-side string secret identifier, encoded as the rich [`SchemaValue::Secret`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRef {
    secret_ref: String,
}

impl SecretRef {
    /// Constructs a secret reference from its opaque string form.
    pub fn new(secret_ref: impl Into<String>) -> Result<Self, FromSchemaError> {
        let secret_ref = secret_ref.into();
        if secret_ref.is_empty() {
            return Err(FromSchemaError::custom(
                "secret identifier must not be empty",
            ));
        }
        Ok(Self { secret_ref })
    }

    pub fn as_str(&self) -> &str {
        &self.secret_ref
    }

    pub fn into_string(self) -> String {
        self.secret_ref
    }
}

impl IntoSchema for SecretRef {
    fn type_id() -> TypeId {
        TypeId::new("golem.core.SecretRef")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::secret(SecretSpec::default())
    }
    fn to_value(&self) -> SchemaValue {
        secret_to_value(self.secret_ref.clone())
    }
}

impl FromSchema for SecretRef {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        Self::new(secret_from_value(v, "SecretRef")?)
    }
}
