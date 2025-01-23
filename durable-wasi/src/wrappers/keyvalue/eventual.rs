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

use crate::bindings::exports::wasi::keyvalue::eventual::{
    BucketBorrow, Error, IncomingValue, Key, OutgoingValueBorrow,
};
use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::keyvalue::eventual::{delete, exists, get, set};
use crate::durability::Durability;
use crate::wrappers::keyvalue::types::{WrappedBucket, WrappedIncomingValue, WrappedOutgoingValue};
use crate::wrappers::keyvalue::wasi_keyvalue_error::WrappedError;
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::keyvalue::eventual::Guest for crate::Component {
    fn get(bucket: BucketBorrow<'_>, key: Key) -> Result<Option<IncomingValue>, Error> {
        let durability = Durability::<Option<Vec<u8>>, SerializableError>::new(
            "golem keyvalue::eventual",
            "get",
            DurableFunctionType::ReadRemote,
        );

        let result = if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let bucket = &wrapped_bucket.bucket;
            let bucket_name = &wrapped_bucket.name;
            let result = get(bucket, &key);
            let (serializable_result, result) = match result {
                Ok(None) => (Ok(None), Ok(None)),
                Ok(Some(incoming_value)) => match incoming_value.incoming_value_consume_sync() {
                    Ok(body) => (Ok(Some(body.clone())), Ok(Some(body))),
                    Err(err) => (Err((&err).into()), Err(err.into())),
                },
                Err(err) => (Err((&err).into()), Err(err.into())),
            };
            let _ = durability.persist_serializable((bucket_name, key), serializable_result);
            result
        } else {
            let data = durability.replay_serializable();
            data.map_err(|err| {
                Error::new(WrappedError::Message {
                    message: err.to_string(),
                })
            })
        };

        match result {
            Ok(None) => Ok(None),
            Ok(Some(data)) => Ok(Some(IncomingValue::new(WrappedIncomingValue::buffered(
                data,
            )))),
            Err(err) => Err(err),
        }
    }

    fn set(
        bucket: BucketBorrow<'_>,
        key: Key,
        outgoing_value: OutgoingValueBorrow<'_>,
    ) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem keyvalue::eventual",
            "set",
            DurableFunctionType::WriteRemote,
        );

        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let outgoing_value = &outgoing_value.get::<WrappedOutgoingValue>().outgoing_value;

            let input = (wrapped_bucket.name.clone(), key.clone(), 0u64); // NOTE: we don't know the outgoing value length here - in the original implementation it was always buffered
            let result =
                set(&wrapped_bucket.bucket, &key, &outgoing_value).map_err(|err| Error::from(err));
            durability.persist(input, result)
        } else {
            durability.replay()
        }
    }

    fn delete(bucket: BucketBorrow<'_>, key: Key) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem keyvalue::eventual",
            "delete",
            DurableFunctionType::WriteRemote,
        );

        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let input = (wrapped_bucket.name.clone(), key.clone());
            let result = delete(&wrapped_bucket.bucket, &key).map_err(|err| Error::from(err));
            durability.persist(input, result)
        } else {
            durability.replay()
        }
    }

    fn exists(bucket: BucketBorrow<'_>, key: Key) -> Result<bool, Error> {
        let durability = Durability::<bool, SerializableError>::new(
            "golem keyvalue::eventual",
            "exists",
            DurableFunctionType::ReadRemote,
        );

        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();

            let input = (wrapped_bucket.name.clone(), key.clone());
            let result = exists(&wrapped_bucket.bucket, &key).map_err(|err| Error::from(err));
            durability.persist(input, result)
        } else {
            durability.replay()
        }
    }
}
