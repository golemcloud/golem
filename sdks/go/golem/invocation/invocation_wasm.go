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

//go:build wasip1

package invocation

import (
	"runtime"
	"time"

	witctx "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_context"
)

// CurrentContext is the invocation context at this point of the invocation.
func CurrentContext() *Context { return &Context{h: wrapContext(witctx.CurrentContext())} }

// StartSpan starts a span named name as a child of the current context.
func StartSpan(name string) *Span {
	raw := witctx.StartSpan(name)
	h := &hostSpan{raw: raw}
	h.cleanup = runtime.AddCleanup(h, func(r *witctx.Span) { r.Drop() }, raw)
	return &Span{h: h}
}

// AllowForwardingTraceContextHeaders turns forwarding of trace context headers
// on outgoing HTTP requests on or off, returning the previous setting.
func AllowForwardingTraceContextHeaders(allow bool) bool {
	return witctx.AllowForwardingTraceContextHeaders(allow)
}

// hostContext owns a context resource, dropped when the wrapper is collected.
type hostContext struct{ raw *witctx.InvocationContext }

func wrapContext(raw *witctx.InvocationContext) *hostContext {
	h := &hostContext{raw: raw}
	runtime.AddCleanup(h, func(r *witctx.InvocationContext) { r.Drop() }, raw)
	return h
}

func (c *hostContext) traceID() string { return c.raw.TraceId() }
func (c *hostContext) spanID() string  { return c.raw.SpanId() }

func (c *hostContext) parent() (contextHost, bool) {
	p := c.raw.Parent()
	if p.IsNone() {
		return nil, false
	}
	return wrapContext(p.Some()), true
}

func (c *hostContext) attribute(key string, inherited bool) (string, bool) {
	v := c.raw.GetAttribute(key, inherited)
	if v.IsNone() {
		return "", false
	}
	return v.Some().String(), true
}

func (c *hostContext) attributes(inherited bool) []Attribute {
	raw := c.raw.GetAttributes(inherited)
	out := make([]Attribute, len(raw))
	for i, a := range raw {
		out[i] = Attribute{Key: a.Key, Value: a.Value.String()}
	}
	return out
}

func (c *hostContext) attributeChain(key string) []string {
	return values(c.raw.GetAttributeChain(key))
}

func (c *hostContext) attributeChains() []AttributeChain {
	raw := c.raw.GetAttributeChains()
	out := make([]AttributeChain, len(raw))
	for i, ch := range raw {
		out[i] = AttributeChain{Key: ch.Key, Values: values(ch.Values)}
	}
	return out
}

func (c *hostContext) traceContextHeaders() [][2]string {
	raw := c.raw.TraceContextHeaders()
	out := make([][2]string, len(raw))
	for i, kv := range raw {
		out[i] = [2]string{kv.F0, kv.F1}
	}
	return out
}

func values(raw []witctx.AttributeValue) []string {
	out := make([]string, len(raw))
	for i, v := range raw {
		out[i] = v.String()
	}
	return out
}

// hostSpan owns a span resource. Finishing drops it; a span that is never
// finished is dropped, and so finished, when collected.
type hostSpan struct {
	raw     *witctx.Span
	cleanup runtime.Cleanup
}

func (s *hostSpan) startedAt() time.Time {
	t := s.raw.StartedAt()
	return time.Unix(t.Seconds, int64(t.Nanoseconds))
}

func (s *hostSpan) setAttributes(attrs []Attribute) {
	raw := make([]witctx.Attribute, len(attrs))
	for i, a := range attrs {
		raw[i] = witctx.Attribute{Key: a.Key, Value: witctx.MakeAttributeValueString(a.Value)}
	}
	s.raw.SetAttributes(raw)
}

func (s *hostSpan) finish() {
	s.cleanup.Stop()
	s.raw.Finish()
	s.raw.Drop()
}
