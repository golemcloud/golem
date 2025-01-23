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

use crate::bindings::exports::wasi::keyvalue::cache::{
    Error, FutureExistsResult, FutureGetOrSetResult, FutureGetResult, FutureResult, GetOrSetEntry,
    Key, OutgoingValue, OutgoingValueBorrow, Pollable, Vacancy,
};
use crate::bindings::exports::wasi::keyvalue::types::IncomingValue;
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::bindings::wasi::keyvalue::cache::{delete, exists, get, get_or_set, set};
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::keyvalue::types::{WrappedIncomingValue, WrappedOutgoingValue};

pub struct WrappedFutureExistsResult {
    future_exists_result: crate::bindings::wasi::keyvalue::cache::FutureExistsResult,
}

impl crate::bindings::exports::wasi::keyvalue::cache::GuestFutureExistsResult
    for WrappedFutureExistsResult
{
    fn future_exists_result_get(&self) -> Option<Result<bool, Error>> {
        observe_function_call("keyvalue::cache::future_exists", "future_exists_result_get");
        let result = self.future_exists_result.future_exists_result_get();
        result.map(|r| r.map_err(|err| err.into()))
    }

    fn listen_to_future_exists_result(&self) -> Pollable {
        observe_function_call(
            "keyvalue::cache::future_exists",
            "listen_to_future_exists_result",
        );
        let pollable = self.future_exists_result.listen_to_future_exists_result();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedFutureExistsResult {
    fn drop(&mut self) {
        observe_function_call("keyvalue::cache::future_exists", "drop");
    }
}

pub struct WrappedVacancy {
    vacancy: crate::bindings::wasi::keyvalue::cache::Vacancy,
}

impl crate::bindings::exports::wasi::keyvalue::cache::GuestVacancy for WrappedVacancy {
    fn vacancy_fill(&self, ttl_ms: Option<u32>) -> OutgoingValue {
        observe_function_call("keyvalue::cache::vacancy", "vacancy_fill");
        let outgoing_value = self.vacancy.vacancy_fill(ttl_ms);
        OutgoingValue::new(WrappedOutgoingValue { outgoing_value })
    }
}

impl Drop for WrappedVacancy {
    fn drop(&mut self) {
        observe_function_call("keyvalue::cache::vacancy", "drop");
    }
}

pub struct WrappedFutureGetOrSetResult {
    future_get_or_set_result: crate::bindings::wasi::keyvalue::cache::FutureGetOrSetResult,
}

impl crate::bindings::exports::wasi::keyvalue::cache::GuestFutureGetOrSetResult
    for WrappedFutureGetOrSetResult
{
    fn future_get_or_set_result_get(&self) -> Option<Result<GetOrSetEntry, Error>> {
        observe_function_call(
            "keyvalue::cache::future_get_or_set",
            "future_get_or_set_result_get",
        );
        let result = self.future_get_or_set_result.future_get_or_set_result_get();
        match result {
            None => None,
            Some(Ok(crate::bindings::wasi::keyvalue::cache::GetOrSetEntry::Occupied(
                incoming_value,
            ))) => Some(Ok(GetOrSetEntry::Occupied(IncomingValue::new(
                WrappedIncomingValue::proxied(incoming_value),
            )))),
            Some(Ok(crate::bindings::wasi::keyvalue::cache::GetOrSetEntry::Vacant(vacancy))) => {
                Some(Ok(GetOrSetEntry::Vacant(Vacancy::new(WrappedVacancy {
                    vacancy,
                }))))
            }
            Some(Err(err)) => Some(Err(err.into())),
        }
    }

    fn listen_to_future_get_or_set_result(&self) -> Pollable {
        observe_function_call(
            "keyvalue::cache::future_get_or_set",
            "listen_to_future_get_or_set_result",
        );
        let pollable = self
            .future_get_or_set_result
            .listen_to_future_get_or_set_result();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedFutureGetOrSetResult {
    fn drop(&mut self) {
        observe_function_call("keyvalue::cache::future_get_or_set", "drop");
    }
}

pub struct WrappedFutureGetResult {
    future_get_result: crate::bindings::wasi::keyvalue::cache::FutureGetResult,
}

impl crate::bindings::exports::wasi::keyvalue::cache::GuestFutureGetResult
    for WrappedFutureGetResult
{
    fn future_get_result_get(&self) -> Option<Result<Option<IncomingValue>, Error>> {
        observe_function_call("keyvalue::cache::future_get", "future_get_result_get");
        let result = self.future_get_result.future_get_result_get();
        match result {
            None => None,
            Some(Err(err)) => Some(Err(err.into())),
            Some(Ok(None)) => Some(Ok(None)),
            Some(Ok(Some(incoming_value))) => Some(Ok(Some(IncomingValue::new(
                WrappedIncomingValue::proxied(incoming_value),
            )))),
        }
    }

    fn listen_to_future_get_result(&self) -> Pollable {
        observe_function_call("keyvalue::cache::future_get", "listen_to_future_get_result");
        let pollable = self.future_get_result.listen_to_future_get_result();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedFutureGetResult {
    fn drop(&mut self) {
        observe_function_call("keyvalue::cache::future_get", "drop");
    }
}

pub struct WrappedFutureResult {
    future_result: crate::bindings::wasi::keyvalue::cache::FutureResult,
}

impl crate::bindings::exports::wasi::keyvalue::cache::GuestFutureResult for WrappedFutureResult {
    fn future_result_get(&self) -> Option<Result<(), Error>> {
        observe_function_call("keyvalue::cache::future_result", "future_result_get");
        let result = self.future_result.future_result_get();
        result.map(|r| r.map_err(|err| err.into()))
    }

    fn listen_to_future_result(&self) -> Pollable {
        observe_function_call("keyvalue::cache::future_result", "listen_to_future_result");
        let pollable = self.future_result.listen_to_future_result();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedFutureResult {
    fn drop(&mut self) {
        observe_function_call("keyvalue::cache::future_result", "drop");
    }
}

impl crate::bindings::exports::wasi::keyvalue::cache::Guest for crate::Component {
    type FutureGetResult = WrappedFutureGetResult;
    type FutureExistsResult = WrappedFutureExistsResult;
    type FutureResult = WrappedFutureResult;
    type FutureGetOrSetResult = WrappedFutureGetOrSetResult;
    type Vacancy = WrappedVacancy;

    fn get(k: Key) -> FutureGetResult {
        observe_function_call("keyvalue::cache", "get");
        let future_get_result = get(&k);
        FutureGetResult::new(WrappedFutureGetResult { future_get_result })
    }

    fn exists(k: Key) -> FutureExistsResult {
        observe_function_call("keyvalue::cache", "exists");
        let future_exists_result = exists(&k);
        FutureExistsResult::new(WrappedFutureExistsResult {
            future_exists_result,
        })
    }

    fn set(k: Key, v: OutgoingValueBorrow<'_>, ttl_ms: Option<u32>) -> FutureResult {
        observe_function_call("keyvalue::cache", "set");
        let v = &v.get::<WrappedOutgoingValue>().outgoing_value;
        let future_result = set(&k, v, ttl_ms);
        FutureResult::new(WrappedFutureResult { future_result })
    }

    fn get_or_set(k: Key) -> FutureGetOrSetResult {
        observe_function_call("keyvalue::cache", "get_or_set");
        let future_get_or_set_result = get_or_set(&k);
        FutureGetOrSetResult::new(WrappedFutureGetOrSetResult {
            future_get_or_set_result,
        })
    }

    fn delete(k: Key) -> FutureResult {
        observe_function_call("keyvalue::cache", "delete");
        let future_result = delete(&k);
        FutureResult::new(WrappedFutureResult { future_result })
    }
}
