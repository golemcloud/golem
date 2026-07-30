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

use super::*;
use crate::durable_host::p3::{DurableP3View, observe_function_call};
use crate::workerctx::WorkerCtx;
use wasmtime::component::Resource;
use wasmtime_wasi_http::p3::WasiHttpView;
use wasmtime_wasi_http::p3::bindings::clocks::monotonic_clock::Duration;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, FieldName, FieldValue, Fields, Headers, Method, Request, RequestOptions, Scheme,
};

pub(super) fn borrow_resource<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

/// Consume a request resource on the replay path, mirroring what the live
/// `WasiHttp::send` does to the request minus the network send.
///
/// The live path deletes the request from the table, converts it with
/// `into_http` (wiring the outgoing body stream and its content-length
/// validation), and drives the body in the background while returning the
/// response head. On replay we never call `send`, so we reproduce the request
/// side here: delete the request, convert it, and spawn a task that drains the
/// body. This
/// * deletes the request resource (matching live), so it does not leak;
/// * reads the guest's outgoing body stream so a guest streaming a body larger
///   than the channel buffer does not block on a reader that never reads;
/// * resolves the guest-held request-body transmission future with the same
///   deterministic result as the live path (e.g. an `HttpRequestBodySize`
///   error for a content-length mismatch), because that validation lives in
///   `into_http`/`GuestBody`, not in the network send.
///
/// The drain runs in a spawned task rather than inline: live `WasiHttp::send`
/// polls its body-I/O future once and, if it is still pending, spawns a task to
/// finish it instead of blocking the response (`p3/host/handler.rs`). Draining
/// inline here would deadlock a guest that awaits the recorded response before
/// finishing its request-body upload.
///
/// The drain's `ErrorCode` is not the `client::send` result — the recorded
/// response head is the authoritative `client::send` outcome — but it feeds the
/// guest-held request-body transmission chain. Live `WasiHttp::send` wires the
/// transmission future to its request I/O result; on replay we wire it to the
/// drain result via a `oneshot` channel, which flows into the `raw_rx` channel
/// of the request's [`PendingHttpRequestBodyTransmission`] wiring.
///
/// The drain result is normally *not* what the guest sees: the guest-facing
/// transmission future resolves from the **recorded** `body-transmission`
/// terminal (see [`HttpRequestBodyTransmissionTask`]), so a live
/// mid-body network failure replays exactly instead of as `Ok(())`. The
/// drain-derived value is the documented best-effort fallback used only when
/// the recorded terminal is missing (the original run crashed after the send
/// `End` but before the upload result was observed): the incomplete
/// `body-transmission` `Start` re-executes against the drain result — which
/// still surfaces deterministic outgoing-body failures such as a
/// `content-length` mismatch or a guest trailers future resolving to an
/// `ErrorCode` (see
/// `request_body_transmission_result_depends_on_unrecorded_body_read`).
impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: HttpError) -> wasmtime::Result<ErrorCode> {
        observe_function_call(&*self.0, "http::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_header_error(&mut self, error: HeaderError) -> wasmtime::Result<types::HeaderError> {
        observe_function_call(&*self.0, "http::types", "convert-header-error");
        types::Host::convert_header_error(&mut WasiHttpView::http(self.0), error)
    }

    fn convert_request_options_error(
        &mut self,
        error: RequestOptionsError,
    ) -> wasmtime::Result<types::RequestOptionsError> {
        observe_function_call(&*self.0, "http::types", "convert-request-options-error");
        types::Host::convert_request_options_error(&mut WasiHttpView::http(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostFields for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "new");
        types::HostFields::new(&mut WasiHttpView::http(self.0))
    }

    fn from_list(
        &mut self,
        entries: Vec<(FieldName, FieldValue)>,
    ) -> HeaderResult<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "from-list");
        types::HostFields::from_list(&mut WasiHttpView::http(self.0), entries)
    }

    fn get(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> wasmtime::Result<Vec<FieldValue>> {
        observe_function_call(&*self.0, "http::types::fields", "get");
        types::HostFields::get(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn has(&mut self, fields: Resource<Fields>, name: FieldName) -> wasmtime::Result<bool> {
        observe_function_call(&*self.0, "http::types::fields", "has");
        types::HostFields::has(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn set(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: Vec<FieldValue>,
    ) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "set");
        types::HostFields::set(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn delete(&mut self, fields: Resource<Fields>, name: FieldName) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "delete");
        types::HostFields::delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn get_and_delete(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
    ) -> HeaderResult<Vec<FieldValue>> {
        observe_function_call(&*self.0, "http::types::fields", "get-and-delete");
        types::HostFields::get_and_delete(&mut WasiHttpView::http(self.0), fields, name)
    }

    fn append(
        &mut self,
        fields: Resource<Fields>,
        name: FieldName,
        value: FieldValue,
    ) -> HeaderResult<()> {
        observe_function_call(&*self.0, "http::types::fields", "append");
        types::HostFields::append(&mut WasiHttpView::http(self.0), fields, name, value)
    }

    fn copy_all(
        &mut self,
        fields: Resource<Fields>,
    ) -> wasmtime::Result<Vec<(FieldName, FieldValue)>> {
        observe_function_call(&*self.0, "http::types::fields", "copy-all");
        types::HostFields::copy_all(&mut WasiHttpView::http(self.0), fields)
    }

    fn clone(&mut self, fields: Resource<Fields>) -> wasmtime::Result<Resource<Fields>> {
        observe_function_call(&*self.0, "http::types::fields", "clone");
        types::HostFields::clone(&mut WasiHttpView::http(self.0), fields)
    }

    fn drop(&mut self, fields: Resource<Fields>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "http::types::fields", "drop");
        types::HostFields::drop(&mut WasiHttpView::http(self.0), fields)
    }
}

impl<Ctx: WorkerCtx> types::HostRequest for DurableP3View<'_, Ctx> {
    fn get_method(&mut self, req: Resource<Request>) -> wasmtime::Result<Method> {
        observe_function_call(&*self.0, "http::types::request", "get-method");
        types::HostRequest::get_method(&mut WasiHttpView::http(self.0), req)
    }

    fn set_method(
        &mut self,
        req: Resource<Request>,
        method: Method,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-method");
        types::HostRequest::set_method(&mut WasiHttpView::http(self.0), req, method)
    }

    fn get_path_with_query(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        observe_function_call(&*self.0, "http::types::request", "get-path-with-query");
        types::HostRequest::get_path_with_query(&mut WasiHttpView::http(self.0), req)
    }

    fn set_path_with_query(
        &mut self,
        req: Resource<Request>,
        path_with_query: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-path-with-query");
        types::HostRequest::set_path_with_query(
            &mut WasiHttpView::http(self.0),
            req,
            path_with_query,
        )
    }

    fn get_scheme(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<Scheme>> {
        observe_function_call(&*self.0, "http::types::request", "get-scheme");
        types::HostRequest::get_scheme(&mut WasiHttpView::http(self.0), req)
    }

    fn set_scheme(
        &mut self,
        req: Resource<Request>,
        scheme: Option<Scheme>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-scheme");
        types::HostRequest::set_scheme(&mut WasiHttpView::http(self.0), req, scheme)
    }

    fn get_authority(&mut self, req: Resource<Request>) -> wasmtime::Result<Option<String>> {
        observe_function_call(&*self.0, "http::types::request", "get-authority");
        types::HostRequest::get_authority(&mut WasiHttpView::http(self.0), req)
    }

    fn set_authority(
        &mut self,
        req: Resource<Request>,
        authority: Option<String>,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::request", "set-authority");
        types::HostRequest::set_authority(&mut WasiHttpView::http(self.0), req, authority)
    }

    fn get_options(
        &mut self,
        req: Resource<Request>,
    ) -> wasmtime::Result<Option<Resource<RequestOptions>>> {
        observe_function_call(&*self.0, "http::types::request", "get-options");
        types::HostRequest::get_options(&mut WasiHttpView::http(self.0), req)
    }

    fn get_headers(&mut self, req: Resource<Request>) -> wasmtime::Result<Resource<Headers>> {
        observe_function_call(&*self.0, "http::types::request", "get-headers");
        types::HostRequest::get_headers(&mut WasiHttpView::http(self.0), req)
    }
}

impl<Ctx: WorkerCtx> types::HostRequestOptions for DurableP3View<'_, Ctx> {
    fn new(&mut self) -> wasmtime::Result<Resource<RequestOptions>> {
        observe_function_call(&*self.0, "http::types::request-options", "new");
        types::HostRequestOptions::new(&mut WasiHttpView::http(self.0))
    }

    fn get_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-connect-timeout",
        );
        types::HostRequestOptions::get_connect_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_connect_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-connect-timeout",
        );
        types::HostRequestOptions::set_connect_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-first-byte-timeout",
        );
        types::HostRequestOptions::get_first_byte_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_first_byte_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-first-byte-timeout",
        );
        types::HostRequestOptions::set_first_byte_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn get_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Option<Duration>> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "get-between-bytes-timeout",
        );
        types::HostRequestOptions::get_between_bytes_timeout(&mut WasiHttpView::http(self.0), opts)
    }

    fn set_between_bytes_timeout(
        &mut self,
        opts: Resource<RequestOptions>,
        duration: Option<Duration>,
    ) -> RequestOptionsResult<()> {
        observe_function_call(
            &*self.0,
            "http::types::request-options",
            "set-between-bytes-timeout",
        );
        types::HostRequestOptions::set_between_bytes_timeout(
            &mut WasiHttpView::http(self.0),
            opts,
            duration,
        )
    }

    fn clone(
        &mut self,
        opts: Resource<RequestOptions>,
    ) -> wasmtime::Result<Resource<RequestOptions>> {
        observe_function_call(&*self.0, "http::types::request-options", "clone");
        types::HostRequestOptions::clone(&mut WasiHttpView::http(self.0), opts)
    }

    fn drop(&mut self, opts: Resource<RequestOptions>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "http::types::request-options", "drop");
        types::HostRequestOptions::drop(&mut WasiHttpView::http(self.0), opts)
    }
}
