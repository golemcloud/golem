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
	"io"
	"reflect"
	"slices"
	"strings"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Typed tool calls.
//
// A declared command is called with its own argument struct. Call starts from
// the declared defaults, lets fill set the rest, and sends the arguments the
// way the host expects them:
//
//	res, err := Commit.Call(func(a *CommitArgs) {
//	    a.Message = "fix the build"
//	    a.Amend = true
//	})
//	if nf, ok := ErrNothingToCommit.Match(err); ok { … }
//
// A command declared with StdoutCommand returns a [ToolInvocation] instead,
// whose standard output is read while the command runs.

// ToolCallErrorKind classifies a failed tool call.
type ToolCallErrorKind uint8

const (
	// ToolCallInvalidInput means the arguments were rejected, by the SDK before
	// sending them or by the tool.
	ToolCallInvalidInput ToolCallErrorKind = iota
	// ToolCallConstraintViolation means the arguments broke one of the
	// command's declared constraints.
	ToolCallConstraintViolation
	// ToolCallDeclaredError means the tool failed with one of its declared
	// error cases; match it with the case's Match.
	ToolCallDeclaredError
	// ToolCallInvalidResult means the tool failed without a declared case, or
	// returned a result that does not decode.
	ToolCallInvalidResult
	// ToolCallUnknownTool means no tool of that name is reachable.
	ToolCallUnknownTool
	// ToolCallUnknownCommand means the tool has no such command.
	ToolCallUnknownCommand
	// ToolCallDenied means the caller may not invoke the tool.
	ToolCallDenied
	// ToolCallCancelled means the call was cancelled before it completed.
	ToolCallCancelled
	// ToolCallResourceExhausted means a quota or limit stopped the call.
	ToolCallResourceExhausted
	// ToolCallProtocolError means the runtime and the tool disagreed on the
	// wire.
	ToolCallProtocolError
	// ToolCallInternalError means the tool's component failed.
	ToolCallInternalError
)

func (k ToolCallErrorKind) String() string {
	switch k {
	case ToolCallInvalidInput:
		return "invalid input"
	case ToolCallConstraintViolation:
		return "constraint violation"
	case ToolCallDeclaredError:
		return "declared error"
	case ToolCallInvalidResult:
		return "invalid result"
	case ToolCallUnknownTool:
		return "unknown tool"
	case ToolCallUnknownCommand:
		return "unknown command"
	case ToolCallDenied:
		return "denied"
	case ToolCallCancelled:
		return "cancelled"
	case ToolCallResourceExhausted:
		return "resource exhausted"
	case ToolCallProtocolError:
		return "protocol error"
	case ToolCallInternalError:
		return "internal error"
	}
	return "failed"
}

// ToolCallError is a failed tool call.
type ToolCallError struct {
	Tool        string
	CommandPath []string
	Kind        ToolCallErrorKind
	// Message is the accompanying detail, empty when there is none.
	Message string
	// ErrorName is the declared error case, for [ToolCallDeclaredError].
	ErrorName string
	payload   TypedValue
	// wire is the tool's own error, kept so a middleware passing the failure
	// on returns it unchanged.
	wire *types.ToolError
}

// Payload returns a declared error's payload; read it with JSON, or decode it
// with [DecodeTypedValue]. A typed caller matches the case with its Match.
func (e *ToolCallError) Payload() TypedValue { return e.payload }

func (e *ToolCallError) Error() string {
	msg := fmt.Sprintf("golem: tool %s %s: %s", e.Tool, commandLabel(e.CommandPath), e.Kind)
	if e.ErrorName != "" {
		msg += " " + e.ErrorName
	}
	if e.Message != "" {
		msg += ": " + e.Message
	}
	return msg
}

// toolCallErrorFromWit classifies the host's tool RPC failure.
func toolCallErrorFromWit(tool string, path []string, e types.ToolRpcError) *ToolCallError {
	out := &ToolCallError{Tool: tool, CommandPath: slices.Clone(path)}
	switch e.Tag() {
	case types.ToolRpcErrorProtocolError:
		out.Kind, out.Message = ToolCallProtocolError, e.ProtocolError()
	case types.ToolRpcErrorDenied:
		out.Kind, out.Message = ToolCallDenied, e.Denied()
	case types.ToolRpcErrorNotFound:
		out.Kind, out.Message = ToolCallUnknownTool, e.NotFound()
	case types.ToolRpcErrorRemoteInternalError:
		out.Kind, out.Message = ToolCallInternalError, e.RemoteInternalError()
	case types.ToolRpcErrorCancelled:
		out.Kind = ToolCallCancelled
	case types.ToolRpcErrorResourceExhausted:
		out.Kind, out.Message = ToolCallResourceExhausted, e.ResourceExhausted()
	case types.ToolRpcErrorRemoteToolError:
		te := e.RemoteToolError()
		out.wire = &te
		switch te.Tag() {
		case types.ToolErrorInvalidToolName:
			out.Kind, out.Message = ToolCallUnknownTool, te.InvalidToolName()
		case types.ToolErrorInvalidCommandPath:
			out.Kind = ToolCallUnknownCommand
		case types.ToolErrorInvalidInput:
			out.Kind, out.Message = ToolCallInvalidInput, te.InvalidInput()
		case types.ToolErrorConstraintViolation:
			out.Kind, out.Message = ToolCallConstraintViolation, te.ConstraintViolation()
		case types.ToolErrorInvalidResult:
			out.Kind, out.Message = ToolCallInvalidResult, te.InvalidResult()
		case types.ToolErrorCustomError:
			ce := te.CustomError()
			out.Kind, out.ErrorName, out.payload = ToolCallDeclaredError, ce.Name, TypedValue{wit: ce.Payload}
		default:
			out.Kind = ToolCallInvalidResult
		}
	default:
		out.Kind = ToolCallInternalError
	}
	return out
}

// toolCall is a started call as the host hands it back: the remote's standard
// output when one was requested, and the pending outcome.
type toolCall struct {
	stdout *byteReader
	stderr *byteReader
	wait   func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError)
	cancel func()
}

// startToolCall starts a call through the host. It is a variable so native
// tests can route calls to a local dispatcher.
var startToolCall = startToolCallHost

// Call runs the command and returns its result. fill sets the arguments on a
// struct that already holds the declared defaults; it may be nil.
func (c *ToolCommand[T, A, O]) Call(fill func(*A)) (O, error) {
	var zero O
	target := c.ce.targetName(c.target)
	call, err := c.ce.start(target, fillArgs(fill))
	if err != nil {
		return zero, err
	}
	out, err := c.ce.finish(target, call)
	if err != nil {
		return zero, err
	}
	o, _ := out.Interface().(O)
	return o, nil
}

// On targets the command at the tool registered under the client's name,
// instead of the definition's own.
func (c *ToolCommand[T, A, O]) On(client *ToolClient[T]) *ToolCommand[T, A, O] {
	return &ToolCommand[T, A, O]{ce: c.ce, target: client.name}
}

// Call starts the command and returns the running invocation, whose standard
// output is read while the command runs. fill is as for [ToolCommand.Call].
func (c *ToolStdoutCommand[T, A, O]) Call(fill func(*A)) (*ToolInvocation[O], error) {
	target := c.ce.targetName(c.target)
	call, err := c.ce.start(target, fillArgs(fill))
	if err != nil {
		return nil, err
	}
	return commandInvocation[O](c.ce, target, call), nil
}

// On targets the command at the tool registered under the client's name,
// instead of the definition's own.
func (c *ToolStdoutCommand[T, A, O]) On(client *ToolClient[T]) *ToolStdoutCommand[T, A, O] {
	return &ToolStdoutCommand[T, A, O]{ce: c.ce, target: client.name}
}

func fillArgs[A any](fill func(*A)) func(reflect.Value) {
	return func(v reflect.Value) {
		if fill != nil {
			fill(v.Addr().Interface().(*A))
		}
	}
}

// targetName is the registration name a call goes to.
func (ce *commandEntry) targetName(override string) string {
	if override != "" {
		return override
	}
	return ce.node.entry.name
}

func (ce *commandEntry) callError(target string, kind ToolCallErrorKind, format string, args ...any) *ToolCallError {
	return &ToolCallError{
		Tool: target, CommandPath: slices.Clone(ce.node.path),
		Kind: kind, Message: fmt.Sprintf(format, args...),
	}
}

func (ce *commandEntry) start(target string, fill func(reflect.Value)) (toolCall, error) {
	input, stdin, err := ce.prepare(target, fill)
	if err != nil {
		return toolCall{}, err
	}
	return startToolCall(target, ce.node.path, input, stdin, ce.stdout)
}

// prepare builds a call's input: the declared defaults, then fill, rendered
// as the canonical input record, and the standard input field.
func (ce *commandEntry) prepare(target string, fill func(reflect.Value)) (types.TypedSchemaValue, io.Reader, error) {
	e := ce.node.entry
	l, ok := ce.resolve()
	if !ok || ce.node.body != ce {
		return types.TypedSchemaValue{}, nil, fmt.Errorf("golem: tool %s command %s is not well-defined:\n%s",
			e.name, ce.label(), allDefErrors(e.d.errs))
	}
	args := reflect.New(ce.argsType).Elem()
	args.Set(l.defaults)
	fill(args)

	var stdin io.Reader
	if l.stdin != nil {
		stdin, _ = args.FieldByIndex(l.stdin.path).Interface().(io.Reader)
		if stdin == nil && !l.stdin.optional {
			return types.TypedSchemaValue{}, nil, ce.callError(target, ToolCallInvalidInput, "the command requires standard input")
		}
	}
	return l.encode(e.d, args), stdin, nil
}

// finish awaits the outcome and decodes the result.
func (ce *commandEntry) finish(target string, call toolCall) (reflect.Value, error) {
	res, rpcErr := call.wait()
	if rpcErr != nil {
		return reflect.Value{}, toolCallErrorFromWit(target, ce.node.path, *rpcErr)
	}
	out := reflect.New(ce.outType).Elem()
	if ce.outType == reflect.TypeFor[Unit]() {
		return out, nil
	}
	if res.IsNone() {
		return reflect.Value{}, ce.callError(target, ToolCallInvalidResult, "the command returned no result")
	}
	tree := res.Some().Value
	dec := decoder{nodes: tree.ValueNodes}
	if err := ce.node.entry.d.compile(ce.outType).decode(&dec, out, tree.Root); err != nil {
		return reflect.Value{}, ce.callError(target, ToolCallInvalidResult, "%v", err)
	}
	return out, nil
}

// ToolInvocation is a running call of a command that writes standard output.
// Read Stdout while the command runs, then Wait for its result; or Collect
// both.
type ToolInvocation[O any] struct {
	call   toolCall
	finish func(toolCall) (O, error)
	done   bool
	out    O
	err    error
}

// commandInvocation is a running call of a declared command, whose result is
// decoded into the command's result type.
func commandInvocation[O any](ce *commandEntry, target string, call toolCall) *ToolInvocation[O] {
	return &ToolInvocation[O]{call: call, finish: func(c toolCall) (O, error) {
		var zero O
		out, err := ce.finish(target, c)
		if err != nil {
			return zero, err
		}
		o, _ := out.Interface().(O)
		return o, nil
	}}
}

// Stdout returns the command's standard output. It ends with io.EOF when the
// command finishes it, and with a [StreamError] when the command fails.
func (i *ToolInvocation[O]) Stdout() io.Reader {
	if i.call.stdout == nil {
		return strings.NewReader("")
	}
	return i.call.stdout
}

// Wait awaits the command's result. The output must be read, or be about to be
// read concurrently, or a command writing more than the stream buffers stalls.
func (i *ToolInvocation[O]) Wait() (O, error) {
	if !i.done {
		i.done = true
		i.out, i.err = i.finish(i.call)
	}
	return i.out, i.err
}

// Cancel asks the runtime to cancel the call.
func (i *ToolInvocation[O]) Cancel() { i.call.cancel() }

// Collect reads the whole output, then awaits the result.
func (i *ToolInvocation[O]) Collect() ([]byte, O, error) {
	data, readErr := io.ReadAll(i.Stdout())
	out, err := i.Wait()
	if err != nil {
		return data, out, err
	}
	return data, out, readErr
}
