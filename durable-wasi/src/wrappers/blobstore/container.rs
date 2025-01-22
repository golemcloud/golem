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

use crate::bindings::exports::wasi::blobstore::container::{
    ContainerMetadata, Error, IncomingValue, ObjectMetadata, ObjectName, OutgoingValueBorrow,
    StreamObjectNames,
};
use crate::bindings::golem::durability::durability::{observe_function_call, DurableFunctionType};
use crate::bindings::wasi::blobstore::blobstore::get_container;
use crate::durability::Durability;
use crate::wrappers::blobstore::types::{WrappedIncomingValue, WrappedOutgoingValue};
use crate::wrappers::blobstore::SerializableObjectMetadata;
use crate::wrappers::SerializableError;
use std::cmp::min;
use std::mem::transmute;
use std::sync::atomic::AtomicUsize;

pub enum WrappedContainer {
    Proxied {
        container: crate::bindings::wasi::blobstore::container::Container,
    },
    Replayed {
        name: String,
        created_at: u64,
        container: crate::bindings::wasi::blobstore::container::Container,
    },
}

impl WrappedContainer {
    pub fn proxied(container: crate::bindings::wasi::blobstore::container::Container) -> Self {
        WrappedContainer::Proxied { container }
    }

    pub fn replayed(name: String, created_at: u64) -> Result<Self, Error> {
        let existing = get_container(&name)?;
        Ok(WrappedContainer::Replayed {
            name,
            created_at,
            container: existing,
        })
    }

    fn inner(&self) -> &crate::bindings::wasi::blobstore::container::Container {
        match self {
            WrappedContainer::Proxied { container } => container,
            WrappedContainer::Replayed { container, .. } => container,
        }
    }

    fn internal_name(&self) -> String {
        match self {
            WrappedContainer::Proxied { container } => container.name().unwrap_or_default(),
            WrappedContainer::Replayed { name, .. } => name.clone(),
        }
    }
}

impl crate::bindings::exports::wasi::blobstore::container::GuestContainer for WrappedContainer {
    fn name(&self) -> Result<String, Error> {
        observe_function_call("blobstore::container::container", "name");
        match self {
            WrappedContainer::Proxied { container } => container.name(),
            WrappedContainer::Replayed { name, .. } => Ok(name.clone()),
        }
    }

    fn info(&self) -> Result<ContainerMetadata, Error> {
        observe_function_call("blobstore::container::container", "info");
        match self {
            WrappedContainer::Proxied { container } => {
                let info = container.info();

                unsafe { transmute(info) }
            }
            WrappedContainer::Replayed {
                name, created_at, ..
            } => Ok(ContainerMetadata {
                name: name.clone(),
                created_at: *created_at,
            }),
        }
    }

    fn get_data(&self, name: ObjectName, start: u64, end: u64) -> Result<IncomingValue, Error> {
        let durability = Durability::<Vec<u8>, SerializableError>::new(
            "golem blobstore::container",
            "get_data",
            DurableFunctionType::ReadRemote,
        );
        let result = if durability.is_live() {
            let result = self.inner().get_data(&name, start, end);

            let data_result = match &result {
                Ok(incoming_data) => incoming_data.incoming_value_consume_sync(),
                Err(err) => Err(err.clone()),
            };
            durability.persist((self.internal_name(), name, start, end), data_result)
        } else {
            durability.replay()
        };

        let data = result?;
        Ok(IncomingValue::new(WrappedIncomingValue::buffered(data)))
    }

    fn write_data(&self, name: ObjectName, data: OutgoingValueBorrow<'_>) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::container",
            "write_data",
            DurableFunctionType::WriteRemote,
        );

        let result = if durability.is_live() {
            let outgoing_value = &data.get::<WrappedOutgoingValue>().outgoing_value;
            let len = 0; // NOTE: this was known in the original blobstore implementation in golem worker executor but not possible to get here
            let result = self.inner().write_data(&name, outgoing_value);

            durability.persist((self.internal_name(), name, len), result)
        } else {
            durability.replay()
        };

        result
    }

    fn list_objects(&self) -> Result<StreamObjectNames, Error> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            "golem blobstore::container",
            "list_object",
            DurableFunctionType::ReadRemote,
        );
        let result = if durability.is_live() {
            let result = self.inner().list_objects();

            let names_result = match &result {
                Ok(stream) => drain_stream(stream),
                Err(err) => Err(err.clone()),
            };

            durability.persist(self.internal_name(), names_result)
        } else {
            durability.replay()
        };

        let names = result?;
        Ok(StreamObjectNames::new(WrappedStreamObjectNames::buffered(
            names,
        )))
    }

    fn delete_object(&self, name: ObjectName) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::container",
            "delete_object",
            DurableFunctionType::WriteRemote,
        );

        let result = if durability.is_live() {
            let result = self.inner().delete_object(&name);
            durability.persist((self.internal_name(), name), result)
        } else {
            durability.replay()
        };

        result
    }

    fn delete_objects(&self, names: Vec<ObjectName>) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::container",
            "delete_objects",
            DurableFunctionType::WriteRemote,
        );

        let result = if durability.is_live() {
            let result = self.inner().delete_objects(&names);
            durability.persist((self.internal_name(), names), result)
        } else {
            durability.replay()
        };

        result
    }

    fn has_object(&self, name: ObjectName) -> Result<bool, Error> {
        let durability = Durability::<bool, SerializableError>::new(
            "golem blobstore::container",
            "has_object",
            DurableFunctionType::ReadRemote,
        );

        let result = if durability.is_live() {
            let result = self.inner().has_object(&name);
            durability.persist((self.internal_name(), name), result)
        } else {
            durability.replay()
        };

        result
    }

    fn object_info(&self, name: ObjectName) -> Result<ObjectMetadata, Error> {
        let durability = Durability::<SerializableObjectMetadata, SerializableError>::new(
            "golem blobstore::container",
            "object_info",
            DurableFunctionType::ReadRemote,
        );

        let result = if durability.is_live() {
            let result = self.inner().object_info(&name);
            let result = result.map(SerializableObjectMetadata::from);
            durability.persist((self.internal_name(), name), result)
        } else {
            durability.replay()
        };

        let result = result.map(|metadata| metadata.into());
        result
    }

    fn clear(&self) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::container",
            "clear",
            DurableFunctionType::WriteRemote,
        );

        let result = if durability.is_live() {
            let result = self.inner().clear();
            durability.persist(self.internal_name(), result)
        } else {
            durability.replay()
        };

        result
    }
}

impl Drop for WrappedContainer {
    fn drop(&mut self) {
        observe_function_call("blobstore::container::container", "drop");
    }
}

pub struct WrappedStreamObjectNames {
    names: Vec<String>,
    position: AtomicUsize,
}

impl WrappedStreamObjectNames {
    pub fn buffered(names: Vec<String>) -> Self {
        WrappedStreamObjectNames {
            names,
            position: AtomicUsize::new(0),
        }
    }
}

impl crate::bindings::exports::wasi::blobstore::container::GuestStreamObjectNames
    for WrappedStreamObjectNames
{
    fn read_stream_object_names(&self, len: u64) -> Result<(Vec<ObjectName>, bool), Error> {
        observe_function_call(
            "blobstore::container::stream_object_names",
            "read_stream_object_names",
        );
        let len = len as usize;
        let position = self
            .position
            .fetch_add(len, std::sync::atomic::Ordering::SeqCst);
        let names = self.names[position..position + len].to_vec();
        Ok((names, position + len < self.names.len()))
    }

    fn skip_stream_object_names(&self, num: u64) -> Result<(u64, bool), Error> {
        observe_function_call(
            "blobstore::container::stream_object_names",
            "skip_stream_object_names",
        );
        let num = num as usize;
        let position = self
            .position
            .fetch_add(num, std::sync::atomic::Ordering::SeqCst);
        Ok((
            min(num, self.names.len() - position) as u64,
            position < self.names.len(),
        ))
    }
}

impl Drop for WrappedStreamObjectNames {
    fn drop(&mut self) {
        observe_function_call("blobstore::container::stream_object_names", "drop");
    }
}

impl crate::bindings::exports::wasi::blobstore::container::Guest for crate::Component {
    type Container = WrappedContainer;
    type StreamObjectNames = WrappedStreamObjectNames;
}

fn drain_stream(
    stream: &crate::bindings::wasi::blobstore::container::StreamObjectNames,
) -> Result<Vec<String>, Error> {
    let mut names = Vec::new();
    loop {
        let (name, more) = stream.read_stream_object_names(1024)?;
        names.extend(name);
        if !more {
            break;
        }
    }
    Ok(names)
}
