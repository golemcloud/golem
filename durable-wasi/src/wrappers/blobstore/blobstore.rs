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

use crate::bindings::exports::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, ObjectId,
};
use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::blobstore::blobstore::{
    container_exists, copy_object, create_container, delete_container, get_container, move_object,
};
use crate::durability::Durability;
use crate::wrappers::blobstore::container::WrappedContainer;
use crate::wrappers::SerializableError;
use std::mem::transmute;

impl From<&crate::bindings::wasi::blobstore::blobstore::Error> for SerializableError {
    fn from(value: &crate::bindings::wasi::blobstore::blobstore::Error) -> Self {
        SerializableError::Generic {
            message: value.clone(),
        }
    }
}

impl From<SerializableError> for crate::bindings::wasi::blobstore::blobstore::Error {
    fn from(value: SerializableError) -> Self {
        value.to_string()
    }
}

impl crate::bindings::exports::wasi::blobstore::blobstore::Guest for crate::Component {
    fn create_container(name: ContainerName) -> Result<Container, Error> {
        let durability = Durability::<u64, SerializableError>::new(
            "golem blobstore::blobstore",
            "create_container",
            DurableFunctionType::WriteRemote,
        );
        let result = if durability.is_live() {
            let result = create_container(&name);
            let create_time: Result<u64, Error> = match &result {
                Ok(container) => container.info().map(|info| info.created_at),
                Err(err) => Err(err.clone()),
            };
            let _ = durability.persist(name, create_time);
            let container = result?;
            WrappedContainer::Proxied { container }
        } else {
            let creation_time =
                durability.replay::<u64, crate::bindings::wasi::blobstore::blobstore::Error>()?;
            WrappedContainer::replayed(name.clone(), creation_time)?
        };
        Ok(Container::new(result))
    }

    fn get_container(name: ContainerName) -> Result<Container, Error> {
        let durability = Durability::<Option<u64>, SerializableError>::new(
            "golem blobstore::blobstore",
            "get_container",
            DurableFunctionType::ReadRemote,
        );
        let result = if durability.is_live() {
            let result = get_container(&name);
            let create_time = match &result {
                Ok(container) => container.info().map(|info| Some(info.created_at)),
                Err(err) => {
                    if err == "Container not found" {
                        Ok(None)
                    } else {
                        Err(err.clone())
                    }
                }
            };
            let _ = durability.persist(name.clone(), create_time);
            let container = result?;
            WrappedContainer::Proxied { container }
        } else {
            let creation_time = durability
                .replay::<Option<u64>, crate::bindings::wasi::blobstore::blobstore::Error>()?;
            match creation_time {
                Some(creation_time) => WrappedContainer::replayed(name.clone(), creation_time)?,
                None => return Err("Container not found".to_string()),
            }
        };
        Ok(Container::new(result))
    }

    fn delete_container(name: ContainerName) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::blobstore",
            "delete_container",
            DurableFunctionType::WriteRemote,
        );
        let result = if durability.is_live() {
            let result = delete_container(&name);
            durability.persist(name, result)
        } else {
            durability.replay()
        };
        result
    }

    fn container_exists(name: ContainerName) -> Result<bool, Error> {
        let durability = Durability::<bool, SerializableError>::new(
            "golem blobstore::blobstore",
            "container_exists",
            DurableFunctionType::ReadRemote,
        );
        let result = if durability.is_live() {
            let result = container_exists(&name);
            durability.persist(name, result)
        } else {
            durability.replay()
        };
        result
    }

    fn copy_object(src: ObjectId, dest: ObjectId) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::blobstore",
            "copy_object",
            DurableFunctionType::WriteRemote,
        );
        let result = if durability.is_live() {
            let input = (
                src.container.clone(),
                src.object.clone(),
                dest.container.clone(),
                dest.object.clone(),
            );
            let src = unsafe { transmute(src) };
            let dest = unsafe { transmute(dest) };
            let result = copy_object(&src, &dest);
            durability.persist(input, result)
        } else {
            durability.replay()
        };
        result
    }

    fn move_object(src: ObjectId, dest: ObjectId) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem blobstore::blobstore",
            "move_object",
            DurableFunctionType::WriteRemote,
        );
        let result = if durability.is_live() {
            let input = (
                src.container.clone(),
                src.object.clone(),
                dest.container.clone(),
                dest.object.clone(),
            );
            let src = unsafe { transmute(src) };
            let dest = unsafe { transmute(dest) };
            let result = move_object(&src, &dest);
            durability.persist(input, result)
        } else {
            durability.replay()
        };
        result
    }
}
