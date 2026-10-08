// Copyright 2024-2026 Golem Cloud
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

//go:build !wasip1

package invocation

// Off the wasm target there is no host; binding the generated calls here would
// drag their //go:wasmimport declarations into a native link.

// CurrentContext is the invocation context at this point of the invocation.
func CurrentContext() *Context { return &Context{} }

// StartSpan starts a span named name as a child of the current context.
func StartSpan(string) *Span { return &Span{} }

// AllowForwardingTraceContextHeaders turns forwarding of trace context headers
// on outgoing HTTP requests on or off, returning the previous setting.
func AllowForwardingTraceContextHeaders(bool) bool { return true }
