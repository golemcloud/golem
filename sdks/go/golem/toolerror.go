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
	"errors"
	"fmt"
	"reflect"
)

// Declared tool errors.
//
// A command's failures are part of its contract: each carries a name, an exit
// code, a kind and an optional typed payload, so a caller can branch on them
// instead of parsing a message.
//
//	type NotFound struct{ Name string }
//
//	var ErrNotFound = golem.DefineToolError[NotFound](Greeter, "not-found",
//	    golem.ToolErrorSpec{Kind: golem.UsageError, ExitCode: 2})
//
//	var Greet = Greeter.Command[GreetArgs, string]("greet", func(a *GreetArgs, s *golem.ToolCommandSpec) {
//	    s.Positional(&a.Name)
//	    s.Raises(ErrNotFound)
//	})
//
//	var _ = Greet.Handle(func(ctx *golem.ToolContext, a GreetArgs) (string, error) {
//	    if !known(a.Name) {
//	        return "", ErrNotFound.New(NotFound{Name: a.Name})
//	    }
//	    return "hi " + a.Name, nil
//	})
//
// A caller recognises the case with Match:
//
//	if nf, ok := ErrNotFound.Match(err); ok { … }

// ToolErrorKind says whose fault a failure is, which is what decides whether a
// caller should fix the invocation or retry it.
type ToolErrorKind uint8

const (
	// UsageError means the invocation itself was wrong.
	UsageError ToolErrorKind = iota
	// RuntimeError means the command ran and failed.
	RuntimeError
)

// ToolErrorSpec describes a declared error case.
type ToolErrorSpec struct {
	Kind ToolErrorKind
	// ExitCode is what a command-line projection of this tool would exit with.
	ExitCode uint8
	// Summary is the one-line description shown in help output.
	Summary string
	// Description is the longer prose for this failure.
	Description string
}

// toolErrorInfo is the registered form of one error case, shared by every
// command that declares it.
type toolErrorInfo struct {
	tool string
	name string
	spec ToolErrorSpec
	// payload is the Go type carried by this error, or nil when it carries none.
	payload reflect.Type
}

// ToolErrorDef is a declared error case. The interface is closed: only
// [DefineToolError] produces one, so [ToolCommandSpec.Raises] cannot be handed
// anything else.
type ToolErrorDef interface{ toolErrorInfo() *toolErrorInfo }

// ToolErrorCase is a declared error case carrying a payload of type P. Use
// [Unit] for a failure that carries none.
type ToolErrorCase[P any, T any] struct{ info *toolErrorInfo }

func (c *ToolErrorCase[P, T]) toolErrorInfo() *toolErrorInfo { return c.info }

// Name returns the error case's declared name.
func (c *ToolErrorCase[P, T]) Name() string { return c.info.name }

// New builds the error a handler returns to fail with this case.
func (c *ToolErrorCase[P, T]) New(payload P) error {
	return &RaisedToolError{info: c.info, payload: reflect.ValueOf(&payload).Elem()}
}

// Match reports whether err is this error case, and its payload if so. It
// recognises both the error a handler returns and the error a typed call
// reports when the tool failed with the case.
func (c *ToolErrorCase[P, T]) Match(err error) (P, bool) {
	var zero P
	var raised *RaisedToolError
	if errors.As(err, &raised) && raised.info == c.info {
		p, _ := raised.payload.Interface().(P)
		return p, true
	}
	var call *ToolCallError
	if !errors.As(err, &call) || call.Kind != ToolCallDeclaredError ||
		call.ErrorName != c.info.name || call.Tool != c.info.tool {
		return zero, false
	}
	if c.info.payload == nil {
		return zero, true
	}
	p, derr := DecodeTypedValue[P](call.payload)
	if derr != nil {
		return zero, false
	}
	return p, true
}

// RaisedToolError is a declared error case together with its payload, produced
// by [ToolErrorCase.New] and recognised by the dispatcher when a handler
// returns it.
type RaisedToolError struct {
	info    *toolErrorInfo
	payload reflect.Value
}

func (e *RaisedToolError) Error() string {
	if e.info.payload == nil {
		return "golem: tool error " + e.info.name
	}
	return fmt.Sprintf("golem: tool error %s: %v", e.info.name, e.payload.Interface())
}

// Name returns the declared name this error was raised under.
func (e *RaisedToolError) Name() string { return e.info.name }

// DefineToolError declares an error case on a tool. Declaring it does not make
// it raisable: each command lists the cases it may return with
// [ToolCommandSpec.Raises], which is what puts them in that command's published
// contract.
func DefineToolError[P any, T any](t *ToolDefinition[T], name string, spec ToolErrorSpec) *ToolErrorCase[P, T] {
	return defineToolErrorOn[P, T](t.entry, name, spec)
}

// defineToolErrorOn records the case on the tool's own entry, which carries the
// registry and definitions it was declared into.
func defineToolErrorOn[P any, T any](e *toolEntry, name string, spec ToolErrorSpec) *ToolErrorCase[P, T] {
	payload := reflect.TypeFor[P]()
	if payload == reflect.TypeFor[Unit]() {
		payload = nil
	}
	info := &toolErrorInfo{tool: e.name, name: name, spec: spec, payload: payload}
	c := &ToolErrorCase[P, T]{info: info}
	switch {
	case name == "":
		e.fail("an error case needs a name")
	case e.errorsByName[name] != nil && !e.remote:
		// A remote tool may give one name different payloads on different
		// commands, and its generated client declares a case for each.
		e.fail("error case already declared: %s", name)
	default:
		e.errorsByName[name] = info
	}
	return c
}
