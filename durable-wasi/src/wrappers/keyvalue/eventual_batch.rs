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

use crate::bindings::exports::wasi::keyvalue::eventual_batch::{
    BucketBorrow, Error, IncomingValue, Key, OutgoingValueBorrow,
};
use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::keyvalue::eventual_batch::{delete_many, get_many, keys, set_many};
use crate::durability::Durability;
use crate::wrappers::keyvalue::types::{WrappedBucket, WrappedIncomingValue, WrappedOutgoingValue};
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::keyvalue::eventual_batch::Guest for crate::Component {
    fn get_many(
        bucket: BucketBorrow<'_>,
        keys: Vec<Key>,
    ) -> Result<Vec<Option<IncomingValue>>, Error> {
        let durability = Durability::<Vec<Option<Vec<u8>>>, SerializableError>::new(
            "golem keyvalue::eventual_batch",
            "get_many",
            DurableFunctionType::ReadRemote,
        );
        let result = if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let input = (wrapped_bucket.name.clone(), keys.clone());
            let result = get_many(&wrapped_bucket.bucket, &keys);
            let (serializable_result, result) = match result {
                Ok(values) => {
                    let mut data = Vec::new();
                    let mut failure = None;
                    for value in values {
                        match value {
                            Some(incoming_value) => {
                                match incoming_value.incoming_value_consume_sync() {
                                    Ok(body) => {
                                        data.push(Some(body.clone()));
                                    }
                                    Err(err) => {
                                        failure = Some(err);
                                        break;
                                    }
                                }
                            }
                            None => {
                                data.push(None);
                            }
                        }
                    }

                    if let Some(err) = failure {
                        (Err((&err).into()), Err(err.into()))
                    } else {
                        (Ok(data.clone()), Ok(data))
                    }
                }
                Err(err) => (Err((&err).into()), Err(err.into())),
            };
            let _ = durability.persist_serializable(input, serializable_result);
            result
        } else {
            let data = durability.replay_serializable();
            data.map_err(|err| {
                Error::new(
                    crate::wrappers::keyvalue::wasi_keyvalue_error::WrappedError::Message {
                        message: err.to_string(),
                    },
                )
            })
        };

        match result {
            Ok(data) => {
                let mut result = Vec::new();
                for maybe_incoming_value in data {
                    match maybe_incoming_value {
                        Some(data) => {
                            result.push(Some(IncomingValue::new(WrappedIncomingValue::buffered(
                                data,
                            ))));
                        }
                        None => {
                            result.push(None);
                        }
                    }
                }
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }

    fn keys(bucket: BucketBorrow<'_>) -> Result<Vec<Key>, Error> {
        let durability = Durability::<Vec<String>, SerializableError>::new(
            "golem keyvalue::eventual_batch",
            "get_keys",
            DurableFunctionType::ReadRemote,
        );
        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let result = keys(&wrapped_bucket.bucket).map_err(|err| Error::from(err));
            durability.persist(wrapped_bucket.name.clone(), result)
        } else {
            durability.replay()
        }
    }

    fn set_many(
        bucket: BucketBorrow<'_>,
        key_values: Vec<(Key, OutgoingValueBorrow<'_>)>,
    ) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem keyvalue::eventual_batch",
            "set_many",
            DurableFunctionType::WriteRemote,
        );

        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let input: (String, Vec<(String, u64)>) = (
                wrapped_bucket.name.clone(),
                key_values.iter().map(|(k, _)| (k.clone(), 0u64)).collect(),
            );
            let inner_key_values: Vec<_> = key_values
                .iter()
                .map(|(k, v)| (k.clone(), v.get::<WrappedOutgoingValue>()))
                .map(|(k, v)| (k, &v.outgoing_value))
                .collect();

            let result =
                set_many(&wrapped_bucket.bucket, &inner_key_values).map_err(|err| Error::from(err));
            durability.persist(input, result)
        } else {
            durability.replay()
        }
    }

    fn delete_many(bucket: BucketBorrow<'_>, keys: Vec<Key>) -> Result<(), Error> {
        let durability = Durability::<(), SerializableError>::new(
            "golem keyvalue::eventual_batch",
            "delete_many",
            DurableFunctionType::WriteRemote,
        );

        if durability.is_live() {
            let wrapped_bucket = bucket.get::<WrappedBucket>();
            let input = (wrapped_bucket.name.clone(), keys.clone());
            let result = delete_many(&wrapped_bucket.bucket, &keys).map_err(|err| Error::from(err));
            durability.persist(input, result)
        } else {
            durability.replay()
        }
    }
}
