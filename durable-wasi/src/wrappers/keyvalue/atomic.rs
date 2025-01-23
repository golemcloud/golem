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

use crate::bindings::exports::wasi::keyvalue::atomic::{BucketBorrow, Error, Key};
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::bindings::wasi::keyvalue::atomic::{compare_and_swap, increment};
use crate::wrappers::keyvalue::types::WrappedBucket;

impl crate::bindings::exports::wasi::keyvalue::atomic::Guest for crate::Component {
    fn increment(bucket: BucketBorrow<'_>, key: Key, delta: u64) -> Result<u64, Error> {
        observe_function_call("keyvalue::atomic", "increment");
        let bucket = &bucket.get::<WrappedBucket>().bucket;
        Ok(increment(bucket, &key, delta)?)
    }

    fn compare_and_swap(bucket: BucketBorrow<'_>, key: Key, old: u64, new: u64) -> Result<bool, Error> {
        observe_function_call("keyvalue::atomic", "compare_and_swap");
        let bucket = &bucket.get::<WrappedBucket>().bucket;
        Ok(compare_and_swap(bucket, &key, old, new)?)
    }
}