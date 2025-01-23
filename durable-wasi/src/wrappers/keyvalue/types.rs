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

use crate::bindings::exports::wasi::keyvalue::types::{
    Bucket, Error, IncomingValueAsyncBody, IncomingValueSyncBody, OutgoingValue,
    OutgoingValueBodyAsync, OutgoingValueBodySync,
};
use crate::bindings::golem::durability::durability::observe_function_call;

pub struct WrappedBucket {
    pub bucket: crate::bindings::wasi::keyvalue::types::Bucket,
    pub name: String,
}

impl crate::bindings::exports::wasi::keyvalue::types::GuestBucket for WrappedBucket {
    fn open_bucket(name: String) -> Result<Bucket, Error> {
        observe_function_call("keyvalue::types::bucket", "open");
        let bucket = crate::bindings::wasi::keyvalue::types::Bucket::open_bucket(&name)?;
        Ok(Bucket::new(WrappedBucket { bucket, name }))
    }
}

impl Drop for WrappedBucket {
    fn drop(&mut self) {
        observe_function_call("keyvalue::types::bucket", "drop");
    }
}

pub struct WrappedIncomingValue {
    pub data: Vec<u8>
}

impl crate::bindings::exports::wasi::keyvalue::types::GuestIncomingValue for WrappedIncomingValue {
    fn incoming_value_consume_sync(&self) -> Result<IncomingValueSyncBody, Error> {
        todo!()
    }

    fn incoming_value_consume_async(&self) -> Result<IncomingValueAsyncBody, Error> {
        todo!()
    }

    fn incoming_value_size(&self) -> Result<u64, Error> {
        todo!()
    }
}

impl Drop for WrappedIncomingValue {
    fn drop(&mut self) {
        todo!()
    }
}

pub struct WrappedOutgoingValue {
    outgoing_value: crate::bindings::wasi::keyvalue::types::OutgoingValue,
}

impl crate::bindings::exports::wasi::keyvalue::types::GuestOutgoingValue for WrappedOutgoingValue {
    fn new_outgoing_value() -> OutgoingValue {
        todo!()
    }

    fn outgoing_value_write_body_async(&self) -> Result<OutgoingValueBodyAsync, Error> {
        todo!()
    }

    fn outgoing_value_write_body_sync(&self, value: OutgoingValueBodySync) -> Result<(), Error> {
        todo!()
    }
}

impl Drop for WrappedOutgoingValue {
    fn drop(&mut self) {
        todo!()
    }
}

impl crate::bindings::exports::wasi::keyvalue::types::Guest for crate::Component {
    type Bucket = WrappedBucket;
    type OutgoingValue = WrappedOutgoingValue;
    type IncomingValue = WrappedIncomingValue;
}
