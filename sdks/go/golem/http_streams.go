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
	"fmt"
	"mime"
	"reflect"
	"strings"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// StreamRoute customizes the Durable Streams HTTP surface of an endpoint whose
// method takes or returns [AgentStream]s. Attach it with [DurableStreams].
//
// The zero value changes nothing: every slot keeps its canonical name and
// content type, and every protocol operation stays allowed.
type StreamRoute struct {
	// Slots renames slots or sets their content type. A slot not listed keeps
	// its defaults.
	Slots []StreamSlot

	// NoExternalWrites stops HTTP clients appending to the input streams.
	NoExternalWrites bool
	// NoStreamDelete stops HTTP clients deleting a stream.
	NoStreamDelete bool
	// NoInvocationDelete stops HTTP clients deleting an invocation.
	NoInvocationDelete bool

	// MaxReadersPerStream caps concurrent long-poll and SSE readers of one
	// stream, from 1 to 16. Zero means no cap.
	MaxReadersPerStream uint32
	// MaxAppendsPerSecond caps append requests per stream and second. Zero means
	// no cap; it cannot be set together with NoExternalWrites.
	MaxAppendsPerSecond uint32
}

// StreamSlot selects one stream slot of the method, by exactly one of Input,
// Output or Result, and sets how it is published.
type StreamSlot struct {
	// Input selects an input field of type AgentStream, by its schema name — the
	// same name [Header] and [Query] bind.
	Input string
	// Output selects an AgentStream field of the returned struct.
	Output string
	// Result selects the returned value itself: a returned AgentStream, or a
	// plain result published as JSON.
	Result bool

	// Name is the public name of the slot in URLs and OpenAPI. The canonical
	// name is then no longer accepted.
	Name string
	// ContentType overrides the content type of a byte stream
	// (AgentStream[byte]). It must be a concrete, non-text, non-JSON type
	// without parameters.
	ContentType string
}

// DurableStreams customizes the endpoint's Durable Streams surface.
func DurableStreams(route StreamRoute) EndpointOpt {
	return func(e *Endpoint) { e.streams = &route; e.streamsCount++ }
}

// maxReadersPerStream is the host's cap on concurrent readers of one stream.
const maxReadersPerStream = 16

// reservedSlotNames are path segments the Durable Streams protocol owns.
var reservedSlotNames = []string{"invocations", "streams", "forks"}

// streamSlots are the stream slots a method has, which a StreamRoute may select.
type streamSlots struct {
	// inputs maps each AgentStream input field to whether it streams bytes.
	inputs map[string]bool
	// outputs maps each AgentStream field of a returned struct to whether it
	// streams bytes.
	outputs map[string]bool
	// result is set when the returned value is a slot of its own.
	result      bool
	resultBytes bool
	resultJSON  bool
}

func (s streamSlots) any() bool {
	return len(s.inputs) > 0 || len(s.outputs) > 0 || (s.result && !s.resultJSON)
}

// methodStreamSlots finds the slots of a method from its Go types.
func methodStreamSlots(in []fieldInfo, out reflect.Type) streamSlots {
	slots := streamSlots{inputs: map[string]bool{}, outputs: map[string]bool{}}
	for _, f := range in {
		if elem, ok := streamElemOf(f.typ); ok {
			slots.inputs[f.name] = elem.Kind() == reflect.Uint8
		}
	}
	if out == nil {
		return slots
	}
	if elem, ok := streamElemOf(out); ok {
		slots.result = true
		slots.resultBytes = elem.Kind() == reflect.Uint8
		return slots
	}
	if out.Kind() == reflect.Struct {
		for i := range out.NumField() {
			f := out.Field(i)
			if f.PkgPath != "" {
				continue
			}
			if elem, ok := streamElemOf(f.Type); ok {
				slots.outputs[lowerFirst(f.Name)] = elem.Kind() == reflect.Uint8
			}
		}
	}
	if len(slots.outputs) == 0 {
		slots.result = true
		slots.resultJSON = true
	}
	return slots
}

// streamElemOf reports the item type of an AgentStream type.
func streamElemOf(t reflect.Type) (reflect.Type, bool) {
	if t.Kind() != reflect.Struct {
		return nil, false
	}
	if s, ok := reflect.New(t).Elem().Interface().(streamish); ok {
		return s.streamElem(), true
	}
	return nil, false
}

// compileStreamRoute validates a StreamRoute against the method's slots and
// compiles it to WIT. The host validates again at deployment; checking here
// reports a mistake where it was made.
func compileStreamRoute(ep Endpoint, slots streamSlots, bound map[string]int) (witTypes.Option[common.DurableStreamRouteOptions], []string) {
	none := witTypes.None[common.DurableStreamRouteOptions]()
	if ep.streams == nil {
		return none, nil
	}
	var errs []string
	fail := func(format string, args ...any) {
		errs = append(errs, fmt.Sprintf("%s %q: DurableStreams: ", ep.method, ep.path)+fmt.Sprintf(format, args...))
	}
	if ep.streamsCount > 1 {
		fail("set %d times (an endpoint has one Durable Streams route)", ep.streamsCount)
	}
	if !slots.any() {
		fail("the method takes and returns no AgentStream")
		return none, errs
	}
	route := ep.streams

	for _, name := range sortedKeys(slots.inputs) {
		if bound[name] > 0 {
			fail("stream input %q cannot be bound to the path, a query parameter or a header", name)
		}
	}

	seen := map[string]bool{}
	public := map[string]bool{}
	out := make([]common.DurableStreamSlotOptions, 0, len(route.Slots))
	for i, slot := range route.Slots {
		selectors := 0
		for _, set := range []bool{slot.Input != "", slot.Output != "", slot.Result} {
			if set {
				selectors++
			}
		}
		if selectors != 1 {
			fail("slot %d must select exactly one of Input, Output or Result", i)
			continue
		}

		var source common.DurableStreamSlotSource
		var key string
		var bytes, json bool
		switch {
		case slot.Input != "":
			isBytes, ok := slots.inputs[slot.Input]
			if !ok {
				fail("Input %q is not an AgentStream input field", slot.Input)
				continue
			}
			source, key, bytes = common.MakeDurableStreamSlotSourceInput(slot.Input), "input:"+slot.Input, isBytes
		case slot.Output != "":
			isBytes, ok := slots.outputs[slot.Output]
			if !ok {
				fail("Output %q is not an AgentStream field of the returned struct", slot.Output)
				continue
			}
			source, key, bytes = common.MakeDurableStreamSlotSourceOutput(slot.Output), "output:"+slot.Output, isBytes
		default:
			if !slots.result {
				fail("Result selects nothing: the method returns stream fields, or nothing")
				continue
			}
			source, key, bytes, json = common.MakeDurableStreamSlotSourceOutput("$result"), "output:$result", slots.resultBytes, slots.resultJSON
		}
		if seen[key] {
			fail("slot %s is listed twice", key)
			continue
		}
		seen[key] = true

		name := witTypes.None[string]()
		if slot.Name != "" {
			if problem := publicSlotNameProblem(slot.Name); problem != "" {
				fail("slot name %q %s", slot.Name, problem)
			} else if public[slot.Name] {
				fail("slot name %q is used twice", slot.Name)
			}
			public[slot.Name] = true
			name = witTypes.Some(slot.Name)
		}
		contentType := witTypes.None[string]()
		if slot.ContentType != "" {
			if problem := contentTypeProblem(slot.ContentType, bytes, json); problem != "" {
				fail("content type %q %s", slot.ContentType, problem)
			}
			contentType = witTypes.Some(slot.ContentType)
		}
		out = append(out, common.DurableStreamSlotOptions{Source: source, Name: name, ContentType: contentType})
	}

	if route.NoExternalWrites && len(slots.inputs) == 0 {
		fail("NoExternalWrites needs an AgentStream input to apply to")
	}
	if route.MaxReadersPerStream > maxReadersPerStream {
		fail("MaxReadersPerStream must be between 1 and %d", maxReadersPerStream)
	}
	if route.MaxAppendsPerSecond > 0 && route.NoExternalWrites {
		fail("MaxAppendsPerSecond cannot be set together with NoExternalWrites")
	}

	denied := func(no bool) witTypes.Option[bool] {
		if no {
			return witTypes.Some(false)
		}
		return witTypes.None[bool]()
	}
	limit := func(v uint32) witTypes.Option[uint32] {
		if v == 0 {
			return witTypes.None[uint32]()
		}
		return witTypes.Some(v)
	}
	load := witTypes.None[common.DurableStreamRouteLoadOptions]()
	if route.MaxReadersPerStream > 0 || route.MaxAppendsPerSecond > 0 {
		load = witTypes.Some(common.DurableStreamRouteLoadOptions{
			MaxConcurrentReadersPerStream:       limit(route.MaxReadersPerStream),
			MaxAppendRequestsPerSecondPerStream: limit(route.MaxAppendsPerSecond),
		})
	}
	return witTypes.Some(common.DurableStreamRouteOptions{
		Slots:                 out,
		AllowExternalWrites:   denied(route.NoExternalWrites),
		AllowStreamDelete:     denied(route.NoStreamDelete),
		AllowInvocationDelete: denied(route.NoInvocationDelete),
		Load:                  load,
	}), errs
}

// publicSlotNameProblem describes why a public slot name cannot be a URL
// segment of the Durable Streams protocol, or returns "".
func publicSlotNameProblem(name string) string {
	for _, reserved := range reservedSlotNames {
		if name == reserved {
			return "is reserved"
		}
	}
	if strings.HasPrefix(name, "__ds") || strings.Contains(name, "$") {
		return "is reserved"
	}
	if len(name) > 64 || name == "." || name == ".." {
		return "is not URL-segment-safe"
	}
	for i := 0; i < len(name); i++ {
		if !slotNameByte(name[i]) {
			return "is not URL-segment-safe"
		}
	}
	return ""
}

func slotNameByte(c byte) bool {
	return c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || strings.IndexByte("-_.~", c) >= 0
}

// contentTypeProblem describes why a content type does not fit a slot, or
// returns "". A byte stream takes a concrete, non-text, non-JSON type; a JSON
// slot only ever travels as application/json.
func contentTypeProblem(value string, bytes, json bool) string {
	parsed, params, err := mime.ParseMediaType(value)
	if err != nil {
		return "is not a valid content type"
	}
	if len(params) > 0 || strings.Contains(parsed, "*") {
		return "cannot have parameters or wildcards"
	}
	isJSON := parsed == "application/json" || strings.HasSuffix(parsed, "+json")
	switch {
	case json:
		if parsed != "application/json" {
			return "does not fit a JSON slot, which is always application/json"
		}
	case bytes:
		if isJSON || strings.HasPrefix(parsed, "text/") {
			return "does not fit a byte stream, which takes a non-text, non-JSON type"
		}
	default:
		if parsed != "application/json" {
			return "applies only to a byte stream (AgentStream[byte])"
		}
	}
	return ""
}
