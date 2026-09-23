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
//	var Greet = golem.Body[GreetArgs, string](Greeter, proto, golem.Raises(ErrNotFound))
//
//	var _ = golem.HandleCommand(Greet, func(ctx *golem.ToolContext, in GreetArgs) string {
//	    if !known(in.Name.Get()) {
//	        panic(ErrNotFound.New(NotFound{Name: in.Name.Get()}))
//	    }
//	    return "hi " + in.Name.Get()
//	})
//
// Raising one is a panic because that is this SDK's abort channel: a declared
// error travels in invoke's error result, which ends the invocation. It is the
// typed counterpart of an anonymous panic, not a value returned in place of a
// result — for an outcome the caller is meant to keep handling, return a
// [Result] instead.

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
// [DefineToolError] produces one, so [Raises] cannot be handed anything else.
type ToolErrorDef interface{ toolErrorInfo() *toolErrorInfo }

// ToolErrorCase is a declared error case carrying a payload of type P. Use
// [Unit] for a failure that carries none.
type ToolErrorCase[P any] struct{ info *toolErrorInfo }

func (c *ToolErrorCase[P]) toolErrorInfo() *toolErrorInfo { return c.info }

// Name returns the error case's declared name.
func (c *ToolErrorCase[P]) Name() string { return c.info.name }

// New builds the error to panic with. It does not itself panic, so the call
// site reads `panic(ErrNotFound.New(...))` and the compiler can see that the
// handler ends there.
func (c *ToolErrorCase[P]) New(payload P) error {
	return &RaisedToolError{info: c.info, payload: reflect.ValueOf(&payload).Elem()}
}

// RaisedToolError is a declared error case together with its payload, produced
// by [ToolErrorCase.New] and recognised by the dispatcher when a handler panics
// with it.
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
// it raisable: each command lists the cases it may raise with [Raises], which is
// what puts them in that command's published contract.
func DefineToolError[P any](t *ToolDefinition, name string, spec ToolErrorSpec) *ToolErrorCase[P] {
	return defineToolErrorInto[P](toolDefs, defs, t, name, spec)
}

func defineToolErrorInto[P any](
	r *toolRegistry, d *definitions, t *ToolDefinition, name string, spec ToolErrorSpec,
) *ToolErrorCase[P] {
	payload := reflect.TypeFor[P]()
	if payload == reflect.TypeFor[Unit]() {
		payload = nil
	}
	info := &toolErrorInfo{tool: t.name, name: name, spec: spec, payload: payload}
	c := &ToolErrorCase[P]{info: info}

	e := r.byName[t.name]
	if e == nil {
		d.recordErr("", "", "error case %q declared on unregistered tool %q", name, t.name)
		return c
	}
	if name == "" {
		d.recordErr("", "", "tool %s: an error case needs a name", t.name)
		return c
	}
	if _, dup := e.errorsByName[name]; dup {
		d.recordErr("", "", "tool %s: error case already declared: %s", t.name, name)
		return c
	}
	e.errorsByName[name] = info
	return c
}

// Raises lists the error cases a command may raise. A command that panics with
// a case it did not list fails as an invalid result, because the case is not
// part of the contract the caller was given.
func Raises(cases ...ToolErrorDef) CommandOpt {
	return func(o *commandOpts) {
		for _, c := range cases {
			o.raises = append(o.raises, c.toolErrorInfo())
		}
	}
}
