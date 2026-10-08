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

package invocation

import "testing"

// Off the wasm target the context is empty and spans record nothing, so code
// that uses them still runs in native tests.
func TestNativeContextAndSpansAreInert(t *testing.T) {
	span := StartSpan("work")
	span.SetAttribute("k", "v")
	span.SetAttributes(Attribute{Key: "a", Value: "b"})
	span.Finish()
	span.Finish()

	ctx := CurrentContext()
	if ctx.TraceID() != "" || ctx.SpanID() != "" {
		t.Errorf("ids %q %q", ctx.TraceID(), ctx.SpanID())
	}
	if _, ok := ctx.Parent(); ok {
		t.Error("a native context has a parent")
	}
	if _, ok := ctx.Attribute("k", true); ok {
		t.Error("a native context has attributes")
	}
	if len(ctx.TraceContextHeaders()) != 0 || ctx.Attributes(true) != nil || ctx.AttributeChains() != nil {
		t.Error("a native context is not empty")
	}
}
