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

//! Affine native streams carried recursively by [`super::SchemaValue`].

use crate::schema::conversion::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder};
use crate::schema::metadata::{MetadataEnvelope, TypeId};
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

#[cfg(all(feature = "guest", not(feature = "host")))]
type RawSchemaValueStream = wit_bindgen::StreamReader<super::wit::wire::SchemaValueTree>;

#[cfg(all(feature = "host", not(feature = "guest")))]
mod active {
    use std::any::Any;
    use std::sync::{Arc, Mutex};

    type OpaqueEndpoint = Box<dyn Any + Send>;

    /// Host-side stream leaf. Clones share one take-once endpoint; they never
    /// duplicate a live stream.
    ///
    /// The endpoint is deliberately type-erased here. The schema model owns
    /// affine transfer semantics, while the embedding runtime owns the relay
    /// protocol used to connect Store-local Component Model streams.
    #[derive(Clone)]
    pub struct SchemaValueStream {
        inner: Arc<Mutex<Option<OpaqueEndpoint>>>,
    }

    impl SchemaValueStream {
        #[doc(hidden)]
        pub fn from_host_endpoint(endpoint: impl Any + Send) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Some(Box::new(endpoint)))),
            }
        }

        #[doc(hidden)]
        pub fn take_host_endpoint<T: Any + Send>(&self) -> Result<T, String> {
            let endpoint = self
                .inner
                .lock()
                .expect("schema value stream mutex poisoned")
                .take()
                .ok_or_else(|| "schema value stream was already transferred".to_string())?;
            endpoint
                .downcast::<T>()
                .map(|endpoint| *endpoint)
                .map_err(|_| {
                    "schema value stream endpoint belongs to an incompatible runtime".to_string()
                })
        }

        #[doc(hidden)]
        pub fn with_host_endpoint<T: Any + Send, R>(
            &self,
            f: impl FnOnce(&T) -> R,
        ) -> Result<R, String> {
            let endpoint = self
                .inner
                .lock()
                .expect("schema value stream mutex poisoned");
            let endpoint = endpoint
                .as_ref()
                .ok_or_else(|| "schema value stream was already transferred".to_string())?;
            endpoint.downcast_ref::<T>().map(f).ok_or_else(|| {
                "schema value stream endpoint belongs to an incompatible runtime".to_string()
            })
        }

        #[doc(hidden)]
        pub fn take_for_transfer(&self) -> Option<Self> {
            self.inner
                .lock()
                .expect("schema value stream mutex poisoned")
                .take()
                .map(|endpoint| Self {
                    inner: Arc::new(Mutex::new(Some(endpoint))),
                })
        }

        #[doc(hidden)]
        pub fn is_present(&self) -> bool {
            self.inner
                .lock()
                .expect("schema value stream mutex poisoned")
                .is_some()
        }

        #[doc(hidden)]
        pub fn cell_id(&self) -> *const () {
            Arc::as_ptr(&self.inner).cast()
        }
    }

    impl std::fmt::Debug for SchemaValueStream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("SchemaValueStream")
                .field(&if self.is_present() {
                    "present"
                } else {
                    "consumed"
                })
                .finish()
        }
    }

    impl PartialEq for SchemaValueStream {
        fn eq(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.inner, &other.inner)
        }
    }

    /// Resource-table representation of `golem:core/types.schema-value-stream`.
    /// It owns one affine runtime endpoint. Component Model readers are
    /// created or consumed only by the embedding runtime while it has access
    /// to the endpoint's Store.
    pub struct SchemaValueStreamHandleRep {
        stream: SchemaValueStream,
    }

    impl SchemaValueStreamHandleRep {
        #[doc(hidden)]
        pub fn new(stream: SchemaValueStream) -> Self {
            Self { stream }
        }

        #[doc(hidden)]
        pub fn into_stream(self) -> SchemaValueStream {
            self.stream
        }
    }
}

#[cfg(all(feature = "guest", not(feature = "host")))]
mod active {
    use super::RawSchemaValueStream;
    use crate::schema::wit::wire;
    use std::sync::{Arc, Mutex, MutexGuard};

    enum State {
        Wrapped(wire::SchemaValueStream),
        Native(RawSchemaValueStream),
    }

    /// Guest-side stream leaf. A leaf can hold the recursive WIT resource or
    /// the native reader obtained from it. Clones share one take-once state.
    #[derive(Clone)]
    pub struct SchemaValueStream {
        inner: Arc<Mutex<Option<State>>>,
    }

    impl SchemaValueStream {
        fn state(&self) -> MutexGuard<'_, Option<State>> {
            self.inner
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
        }

        #[doc(hidden)]
        pub fn from_native(reader: RawSchemaValueStream) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Some(State::Native(reader)))),
            }
        }

        #[doc(hidden)]
        pub fn from_wrapped(stream: wire::SchemaValueStream) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Some(State::Wrapped(stream)))),
            }
        }

        #[doc(hidden)]
        pub fn is_present(&self) -> bool {
            self.state().is_some()
        }

        #[doc(hidden)]
        pub fn is_wrapped(&self) -> bool {
            matches!(&*self.state(), Some(State::Wrapped(_)))
        }

        #[doc(hidden)]
        pub fn cell_id(&self) -> *const () {
            Arc::as_ptr(&self.inner).cast()
        }

        #[doc(hidden)]
        pub fn take_wrapped(&self) -> Option<wire::SchemaValueStream> {
            let mut state = self.state();
            match state.take() {
                Some(State::Wrapped(stream)) => Some(stream),
                Some(native @ State::Native(_)) => {
                    *state = Some(native);
                    None
                }
                None => None,
            }
        }

        #[doc(hidden)]
        pub async fn take_wrapped_async(&self) -> Result<wire::SchemaValueStream, String> {
            let state = self
                .state()
                .take()
                .ok_or_else(|| "schema value stream was already transferred".to_string())?;
            Ok(match state {
                State::Wrapped(stream) => stream,
                State::Native(reader) => wire::SchemaValueStream::wrap(reader).await,
            })
        }

        #[doc(hidden)]
        pub async fn ensure_wrapped(&self) -> Result<(), String> {
            let state = self
                .state()
                .take()
                .ok_or_else(|| "schema value stream was already transferred".to_string())?;
            *self.state() = Some(match state {
                State::Wrapped(stream) => State::Wrapped(stream),
                State::Native(reader) => {
                    State::Wrapped(wire::SchemaValueStream::wrap(reader).await)
                }
            });
            Ok(())
        }

        #[doc(hidden)]
        pub async fn take_native(self) -> Result<RawSchemaValueStream, String> {
            let state = self
                .state()
                .take()
                .ok_or_else(|| "schema value stream was already transferred".to_string())?;
            Ok(match state {
                State::Wrapped(stream) => wire::SchemaValueStream::unwrap(stream).await,
                State::Native(reader) => reader,
            })
        }

        #[doc(hidden)]
        pub async fn next_wire(&self) -> Result<Option<wire::SchemaValueTree>, String> {
            let state = self.state().take().ok_or_else(|| {
                "schema value stream is already in use or was transferred".to_string()
            })?;
            let mut reader = match state {
                State::Wrapped(stream) => wire::SchemaValueStream::unwrap(stream).await,
                State::Native(reader) => reader,
            };
            let item = reader.next().await;
            *self.state() = Some(State::Native(reader));
            Ok(item)
        }
    }

    impl std::fmt::Debug for SchemaValueStream {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let state = match &*self.state() {
                Some(State::Wrapped(_)) => "wrapped",
                Some(State::Native(_)) => "native",
                None => "consumed",
            };
            f.debug_tuple("SchemaValueStream").field(&state).finish()
        }
    }

    impl PartialEq for SchemaValueStream {
        fn eq(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.inner, &other.inner)
        }
    }
}

#[cfg(any(
    all(feature = "host", not(feature = "guest")),
    all(feature = "guest", not(feature = "host"))
))]
pub use active::SchemaValueStream;

#[cfg(all(feature = "host", not(feature = "guest")))]
pub use active::SchemaValueStreamHandleRep;

#[cfg(not(any(
    all(feature = "host", not(feature = "guest")),
    all(feature = "guest", not(feature = "host"))
)))]
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaValueStream(());

impl serde::Serialize for SchemaValueStream {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom(
            "live schema value streams cannot be serialized",
        ))
    }
}

impl<'de> serde::Deserialize<'de> for SchemaValueStream {
    fn deserialize<D: serde::Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "live schema value streams cannot be deserialized",
        ))
    }
}

impl IntoSchema for SchemaValueStream {
    fn type_id() -> TypeId {
        TypeId::new("golem.core.SchemaValueStream")
    }

    fn register_in(_builder: &mut SchemaBuilder) -> SchemaType {
        SchemaType::Stream {
            inner: None,
            metadata: MetadataEnvelope::default(),
        }
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Stream(self.clone())
    }
}

impl FromSchema for SchemaValueStream {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::Stream(stream) => Ok(stream.clone()),
            other => Err(FromSchemaError::shape_mismatch(
                "stream",
                format!("{other:?}"),
                "SchemaValueStream",
            )),
        }
    }
}
