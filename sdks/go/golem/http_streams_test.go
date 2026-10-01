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

package golem

import (
	"reflect"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

type streamedEvents struct {
	Events AgentStream[string]
	Audit  AgentStream[byte]
}

// streamMethod is a method taking a byte stream "uploads" and a plain "label",
// returning out.
func streamMethod(out reflect.Type, eps ...Endpoint) *methodEntry {
	m := method("events", []fieldInfo{
		{name: "uploads", typ: reflect.TypeFor[AgentStream[byte]]()},
		{name: "label", typ: reflect.TypeFor[string]()},
	}, eps...)
	m.outType = out
	return m
}

func compileStreams(t *testing.T, out reflect.Type, route StreamRoute) (common.DurableStreamRouteOptions, []definitionError) {
	t.Helper()
	e := agent("Media", &Mount{Path: "/media"}, nil,
		streamMethod(out, POST("/events", DurableStreams(route))))
	_, eps, errs := buildHTTP(e)
	det := eps["events"][0].DurableStreams
	if det.IsNone() {
		return common.DurableStreamRouteOptions{}, errs
	}
	return det.Some(), errs
}

func TestDurableStreamsCompilesTheRoute(t *testing.T) {
	opts, errs := compileStreams(t, reflect.TypeFor[AgentStream[byte]](), StreamRoute{
		Slots: []StreamSlot{
			{Input: "uploads", Name: "uploads-in", ContentType: "application/vnd.example.bin"},
			{Result: true, Name: "events"},
		},
		NoStreamDelete:      true,
		MaxReadersPerStream: 8,
		MaxAppendsPerSecond: 25,
	})
	if len(errs) > 0 {
		t.Fatalf("unexpected errors: %v", errs)
	}
	if len(opts.Slots) != 2 {
		t.Fatalf("slots = %+v", opts.Slots)
	}
	if s := opts.Slots[0]; s.Source.Tag() != common.DurableStreamSlotSourceInput || s.Source.Input() != "uploads" ||
		s.Name.Some() != "uploads-in" || s.ContentType.Some() != "application/vnd.example.bin" {
		t.Fatalf("input slot = %+v", s)
	}
	if s := opts.Slots[1]; s.Source.Tag() != common.DurableStreamSlotSourceOutput || s.Source.Output() != "$result" {
		t.Fatalf("result slot = %+v", s)
	}
	// Only what was changed is sent; the host applies its own defaults.
	if opts.AllowExternalWrites.IsSome() || opts.AllowInvocationDelete.IsSome() {
		t.Fatalf("defaults must travel as none: %+v", opts)
	}
	if opts.AllowStreamDelete.Some() != false {
		t.Fatalf("NoStreamDelete must send false")
	}
	load := opts.Load.Some()
	if load.MaxConcurrentReadersPerStream.Some() != 8 || load.MaxAppendRequestsPerSecondPerStream.Some() != 25 {
		t.Fatalf("load = %+v", load)
	}
}

func TestAnEndpointWithoutDurableStreamsSendsNone(t *testing.T) {
	e := agent("Media", &Mount{Path: "/media"}, nil, streamMethod(nil, POST("/events")))
	_, eps, errs := buildHTTP(e)
	if len(errs) > 0 || eps["events"][0].DurableStreams.IsSome() {
		t.Fatalf("errs = %v, streams = %+v", errs, eps["events"][0].DurableStreams)
	}
}

// Output selects a stream field of a returned struct; Result is then not a slot.
func TestDurableStreamsSelectsReturnedStructFields(t *testing.T) {
	out := reflect.TypeFor[streamedEvents]()
	opts, errs := compileStreams(t, out, StreamRoute{Slots: []StreamSlot{
		{Output: "events", Name: "live"},
		{Output: "audit", ContentType: "application/octet-stream"},
	}})
	if len(errs) > 0 || len(opts.Slots) != 2 || opts.Slots[0].Source.Output() != "events" {
		t.Fatalf("errs = %v, slots = %+v", errs, opts.Slots)
	}
	_, errs = compileStreams(t, out, StreamRoute{Slots: []StreamSlot{{Result: true}}})
	if !anyErrContains(errs, "Result selects nothing") {
		t.Fatalf("got %v", errs)
	}
}

func TestDurableStreamsRejectsMisuse(t *testing.T) {
	byteStream := reflect.TypeFor[AgentStream[byte]]()
	stringStream := reflect.TypeFor[AgentStream[string]]()
	cases := []struct {
		name  string
		out   reflect.Type
		route StreamRoute
		want  string
	}{
		{"two selectors", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", Result: true}}}, "exactly one of"},
		{"no selector", byteStream, StreamRoute{Slots: []StreamSlot{{Name: "x"}}}, "exactly one of"},
		{"input not a stream", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "label"}}}, `Input "label" is not an AgentStream`},
		{"unknown output", byteStream, StreamRoute{Slots: []StreamSlot{{Output: "nope"}}}, `Output "nope"`},
		{"listed twice", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads"}, {Input: "uploads"}}}, "listed twice"},
		{"reserved name", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", Name: "streams"}}}, "is reserved"},
		{"dollar name", byteStream, StreamRoute{Slots: []StreamSlot{{Result: true, Name: "$x"}}}, "is reserved"},
		{"unsafe name", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", Name: "a/b"}}}, "URL-segment-safe"},
		{"duplicate public name", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", Name: "x"}, {Result: true, Name: "x"}}}, "used twice"},
		{"text on bytes", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", ContentType: "text/plain"}}}, "does not fit a byte stream"},
		{"json on bytes", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", ContentType: "application/x+json"}}}, "does not fit a byte stream"},
		{"parameters", byteStream, StreamRoute{Slots: []StreamSlot{{Input: "uploads", ContentType: "application/x; a=b"}}}, "parameters or wildcards"},
		{"type on a string stream", stringStream, StreamRoute{Slots: []StreamSlot{{Result: true, ContentType: "application/x"}}}, "applies only to a byte stream"},
		{"too many readers", byteStream, StreamRoute{MaxReadersPerStream: 17}, "between 1 and 16"},
		{"appends without writes", byteStream, StreamRoute{NoExternalWrites: true, MaxAppendsPerSecond: 5}, "cannot be set together"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			_, errs := compileStreams(t, c.out, c.route)
			if !anyErrContains(errs, c.want) {
				t.Fatalf("want an error mentioning %q, got %v", c.want, errs)
			}
		})
	}
}

func TestDurableStreamsNeedsAStreamMethod(t *testing.T) {
	m := method("plain", fields("label"), POST("/plain", DurableStreams(StreamRoute{})))
	m.outType = reflect.TypeFor[string]()
	_, _, errs := buildHTTP(agent("A", &Mount{Path: "/a"}, nil, m))
	if !anyErrContains(errs, "takes and returns no AgentStream") {
		t.Fatalf("got %v", errs)
	}
}

func TestAStreamInputCannotBeBoundFromTheRequestLine(t *testing.T) {
	e := agent("Media", &Mount{Path: "/media"}, nil,
		streamMethod(nil, POST("/events?u={uploads}", DurableStreams(StreamRoute{}))))
	_, _, errs := buildHTTP(e)
	if !anyErrContains(errs, `stream input "uploads" cannot be bound`) {
		t.Fatalf("got %v", errs)
	}
}

func TestNoExternalWritesNeedsAnInputStream(t *testing.T) {
	m := method("out", fields("label"), POST("/out", DurableStreams(StreamRoute{NoExternalWrites: true})))
	m.outType = reflect.TypeFor[AgentStream[string]]()
	_, _, errs := buildHTTP(agent("A", &Mount{Path: "/a"}, nil, m))
	if !anyErrContains(errs, "NoExternalWrites needs") {
		t.Fatalf("got %v", errs)
	}
}
