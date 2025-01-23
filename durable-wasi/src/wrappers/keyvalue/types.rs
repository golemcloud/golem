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

use crate::bindings::exports::wasi::io::streams::OutputStream;
use crate::bindings::exports::wasi::keyvalue::types::{
    Bucket, Error, IncomingValueAsyncBody, IncomingValueSyncBody, OutgoingValue,
    OutgoingValueBodyAsync, OutgoingValueBodySync,
};
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::wrappers::io::streams::{WrappedInputStream, WrappedOutputStream};

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

pub enum WrappedIncomingValue {
    Proxied {
        incoming_value: crate::bindings::wasi::keyvalue::types::IncomingValue,
    },
    Buffered {
        data: Vec<u8>,
    },
}

impl WrappedIncomingValue {
    pub fn proxied(incoming_value: crate::bindings::wasi::keyvalue::types::IncomingValue) -> Self {
        WrappedIncomingValue::Proxied { incoming_value }
    }

    pub fn buffered(data: Vec<u8>) -> Self {
        WrappedIncomingValue::Buffered { data }
    }
}

impl crate::bindings::exports::wasi::keyvalue::types::GuestIncomingValue for WrappedIncomingValue {
    fn incoming_value_consume_sync(&self) -> Result<IncomingValueSyncBody, Error> {
        observe_function_call(
            "keyvalue::types::incoming_value",
            "incoming_value_consume_sync",
        );
        match self {
            WrappedIncomingValue::Proxied { incoming_value } => {
                let data = incoming_value.incoming_value_consume_sync()?;
                Ok(data)
            }
            WrappedIncomingValue::Buffered { data } => Ok(data.clone()),
        }
    }

    fn incoming_value_consume_async(&self) -> Result<IncomingValueAsyncBody, Error> {
        observe_function_call(
            "keyvalue::types::incoming_value",
            "incoming_value_consume_async",
        );
        match self {
            WrappedIncomingValue::Proxied { incoming_value } => {
                let input_stream = incoming_value.incoming_value_consume_async()?;
                Ok(IncomingValueAsyncBody::new(WrappedInputStream::proxied(
                    input_stream,
                )))
            }
            WrappedIncomingValue::Buffered { data } => Ok(IncomingValueAsyncBody::new(
                WrappedInputStream::buffered(data.clone()),
            )),
        }
    }

    fn incoming_value_size(&self) -> Result<u64, Error> {
        observe_function_call("keyvalue::types::incoming_value", "size");
        match self {
            WrappedIncomingValue::Proxied { incoming_value } => {
                let size = incoming_value.incoming_value_size()?;
                Ok(size)
            }
            WrappedIncomingValue::Buffered { data } => Ok(data.len() as u64),
        }
    }
}

impl Drop for WrappedIncomingValue {
    fn drop(&mut self) {
        observe_function_call("keyvalue::types::incoming_value", "drop");
    }
}

pub struct WrappedOutgoingValue {
    pub outgoing_value: crate::bindings::wasi::keyvalue::types::OutgoingValue,
}

impl crate::bindings::exports::wasi::keyvalue::types::GuestOutgoingValue for WrappedOutgoingValue {
    fn new_outgoing_value() -> OutgoingValue {
        observe_function_call("keyvalue::types::outgoing_value", "new_outgoing_value");
        let outgoing_value =
            crate::bindings::wasi::keyvalue::types::OutgoingValue::new_outgoing_value();
        OutgoingValue::new(WrappedOutgoingValue { outgoing_value })
    }

    fn outgoing_value_write_body_async(&self) -> Result<OutgoingValueBodyAsync, Error> {
        observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_async",
        );
        let output_stream = self.outgoing_value.outgoing_value_write_body_async()?;
        Ok(OutputStream::new(WrappedOutputStream { output_stream }))
    }

    fn outgoing_value_write_body_sync(&self, value: OutgoingValueBodySync) -> Result<(), Error> {
        observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_sync",
        );
        Ok(self.outgoing_value.outgoing_value_write_body_sync(&value)?)
    }
}

impl Drop for WrappedOutgoingValue {
    fn drop(&mut self) {
        observe_function_call("keyvalue::types::outgoing_value", "drop");
    }
}

impl crate::bindings::exports::wasi::keyvalue::types::Guest for crate::Component {
    type Bucket = WrappedBucket;
    type OutgoingValue = WrappedOutgoingValue;
    type IncomingValue = WrappedIncomingValue;
}
