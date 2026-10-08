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

// Package invocation reads the invocation context Golem associates with every
// invocation, and starts custom spans with attributes in it. The context is
// inherited by agent-to-agent calls; its trace context travels on outgoing
// HTTP requests.
//
//	span := invocation.StartSpan("charge")
//	defer span.Finish()
//	span.SetAttribute("order", order.ID)
//
//	ctx := invocation.CurrentContext()
//	slog.Info("charging", "trace", ctx.TraceID())
//
// Off the wasm target there is no host: the context is empty and spans record
// nothing, so code that uses them still runs in native tests.
package invocation

import (
	"net/http"
	"time"
)

// Attribute is a key and its value in a span.
type Attribute struct {
	Key   string
	Value string
}

// AttributeChain is every value a key has along the span stack, the innermost
// first.
type AttributeChain struct {
	Key    string
	Values []string
}

// Context is a snapshot of the invocation context: spans started afterwards
// are not reflected in it.
type Context struct{ h contextHost }

type contextHost interface {
	traceID() string
	spanID() string
	parent() (contextHost, bool)
	attribute(key string, inherited bool) (string, bool)
	attributes(inherited bool) []Attribute
	attributeChain(key string) []string
	attributeChains() []AttributeChain
	traceContextHeaders() [][2]string
}

// TraceID is the trace the invocation belongs to, from an incoming trace header
// or generated at the edge of Golem.
func (c *Context) TraceID() string {
	if c.h == nil {
		return ""
	}
	return c.h.traceID()
}

// SpanID is the current span's id.
func (c *Context) SpanID() string {
	if c.h == nil {
		return ""
	}
	return c.h.spanID()
}

// Parent is the enclosing context; ok is false at the root.
func (c *Context) Parent() (parent *Context, ok bool) {
	if c.h == nil {
		return nil, false
	}
	p, ok := c.h.parent()
	if !ok {
		return nil, false
	}
	return &Context{h: p}, true
}

// Attribute is the value of key in the current span or, when inherited is true,
// the innermost value along the span stack.
func (c *Context) Attribute(key string, inherited bool) (value string, ok bool) {
	if c.h == nil {
		return "", false
	}
	return c.h.attribute(key, inherited)
}

// Attributes are the current span's attributes or, when inherited is true, the
// merged set along the span stack, each key with its innermost value.
func (c *Context) Attributes(inherited bool) []Attribute {
	if c.h == nil {
		return nil
	}
	return c.h.attributes(inherited)
}

// AttributeChain is every value of key along the span stack, the innermost
// first; empty when no span has it.
func (c *Context) AttributeChain(key string) []string {
	if c.h == nil {
		return nil
	}
	return c.h.attributeChain(key)
}

// AttributeChains is every key with all of its values along the span stack.
func (c *Context) AttributeChains() []AttributeChain {
	if c.h == nil {
		return nil
	}
	return c.h.attributeChains()
}

// TraceContextHeaders are the W3C Trace Context headers of the context, ready
// to add to a request.
func (c *Context) TraceContextHeaders() http.Header {
	h := http.Header{}
	if c.h == nil {
		return h
	}
	for _, kv := range c.h.traceContextHeaders() {
		h.Add(kv[0], kv[1])
	}
	return h
}

// Span is a custom unit of work in the invocation context. Finish ends it; one
// that is never finished ends when it is garbage collected.
type Span struct{ h spanHost }

type spanHost interface {
	startedAt() time.Time
	setAttributes(attrs []Attribute)
	finish()
}

// StartedAt is when the span started.
func (s *Span) StartedAt() time.Time {
	if s.h == nil {
		return time.Time{}
	}
	return s.h.startedAt()
}

// SetAttribute sets one attribute on the span.
func (s *Span) SetAttribute(key, value string) { s.SetAttributes(Attribute{Key: key, Value: value}) }

// SetAttributes sets several attributes on the span.
func (s *Span) SetAttributes(attrs ...Attribute) {
	if s.h == nil || len(attrs) == 0 {
		return
	}
	s.h.setAttributes(attrs)
}

// Finish ends the span; finishing it again does nothing.
func (s *Span) Finish() {
	if s.h == nil {
		return
	}
	s.h.finish()
	s.h = nil
}
