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

use crate::bindings::exports::wasi::blobstore::types::{
    Error, IncomingValueAsyncBody, IncomingValueSyncBody, OutgoingValue, OutputStream,
};
use crate::bindings::exports::wasi::io::streams::InputStream;
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::wrappers::io::streams::{WrappedInputStream, WrappedOutputStream};

pub struct WrappedIncomingValue {
    data: Vec<u8>,
}

impl WrappedIncomingValue {
    pub fn buffered(data: Vec<u8>) -> Self {
        WrappedIncomingValue { data }
    }
}

impl crate::bindings::exports::wasi::blobstore::types::GuestIncomingValue for WrappedIncomingValue {
    fn incoming_value_consume_sync(&self) -> Result<IncomingValueSyncBody, Error> {
        observe_function_call(
            "blobstore::types::incoming_value",
            "incoming_value_consume_sync",
        );
        Ok(self.data.clone())
    }

    fn incoming_value_consume_async(&self) -> Result<IncomingValueAsyncBody, Error> {
        observe_function_call(
            "blobstore::types::incoming_value",
            "incoming_value_consume_async",
        );
        Ok(InputStream::new(WrappedInputStream::buffered(
            self.data.clone(),
        )))
    }

    fn size(&self) -> u64 {
        observe_function_call("blobstore::types::incoming_value", "size");
        self.data.len() as u64
    }
}

pub struct WrappedOutgoingValue {
    pub outgoing_value: crate::bindings::wasi::blobstore::types::OutgoingValue,
}

impl crate::bindings::exports::wasi::blobstore::types::GuestOutgoingValue for WrappedOutgoingValue {
    fn new_outgoing_value() -> OutgoingValue {
        observe_function_call("blobstore::types::outgoing_value", "new_outgoing_value");
        let outgoing_value =
            crate::bindings::wasi::blobstore::types::OutgoingValue::new_outgoing_value();
        OutgoingValue::new(WrappedOutgoingValue { outgoing_value })
    }

    fn outgoing_value_write_body(&self) -> Result<OutputStream, ()> {
        observe_function_call(
            "blobstore::types::outgoing_value",
            "outgoing_value_write_body",
        );
        let output_stream = self.outgoing_value.outgoing_value_write_body()?;
        Ok(OutputStream::new(WrappedOutputStream { output_stream }))
    }
}

impl Drop for WrappedOutgoingValue {
    fn drop(&mut self) {
        observe_function_call("blobstore::types::outgoing_value", "drop");
    }
}

impl crate::bindings::exports::wasi::blobstore::types::Guest for crate::Component {
    type OutgoingValue = WrappedOutgoingValue;
    type IncomingValue = WrappedIncomingValue;
}
