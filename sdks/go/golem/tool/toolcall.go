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

package tool

import (
	"errors"
	"fmt"
	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
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
// A command declared with OutputCommand returns a [ToolInvocation] instead,
// whose standard output and standard error are read while the command runs.

// CallErrorKind classifies a failed tool call.
type CallErrorKind uint8

const (
	// CallInvalidInput means the arguments were rejected, by the SDK before
	// sending them or by the tool.
	CallInvalidInput CallErrorKind = iota
	// CallConstraintViolation means the arguments broke one of the
	// command's declared constraints.
	CallConstraintViolation
	// CallDeclaredError means the tool failed with one of its declared
	// error cases; match it with the case's Match.
	CallDeclaredError
	// CallInvalidResult means the tool failed without a declared case, or
	// returned a result that does not decode.
	CallInvalidResult
	// CallUnknownTool means no tool of that name is reachable.
	CallUnknownTool
	// CallUnknownCommand means the tool has no such command.
	CallUnknownCommand
	// CallDenied means the caller may not invoke the tool.
	CallDenied
	// CallCancelled means the call was cancelled before it completed.
	CallCancelled
	// CallResourceExhausted means a quota or limit stopped the call.
	CallResourceExhausted
	// CallProtocolError means the runtime and the tool disagreed on the
	// wire.
	CallProtocolError
	// CallInternalError means the tool's component failed.
	CallInternalError
)

func (k CallErrorKind) String() string {
	switch k {
	case CallInvalidInput:
		return "invalid input"
	case CallConstraintViolation:
		return "constraint violation"
	case CallDeclaredError:
		return "declared error"
	case CallInvalidResult:
		return "invalid result"
	case CallUnknownTool:
		return "unknown tool"
	case CallUnknownCommand:
		return "unknown command"
	case CallDenied:
		return "denied"
	case CallCancelled:
		return "cancelled"
	case CallResourceExhausted:
		return "resource exhausted"
	case CallProtocolError:
		return "protocol error"
	case CallInternalError:
		return "internal error"
	}
	return "failed"
}

// CallError is a failed tool call.
type CallError struct {
	Tool        string
	CommandPath []string
	Kind        CallErrorKind
	// Message is the accompanying detail, empty when there is none.
	Message string
	// ErrorName is the declared error case, for [ToolCallDeclaredError].
	ErrorName string
	payload   golem.TypedValue
	// wire is the tool's own error, kept so a middleware passing the failure
	// on returns it unchanged.
	wire *types.ToolError
}

// Payload returns a declared error's payload; read it with JSON, or decode it
// with [DecodeTypedValue]. A typed caller matches the case with its Match.
func (e *CallError) Payload() golem.TypedValue { return e.payload }

// InvalidInput rejects a tool invocation's arguments, from a command handler
// or a middleware; the caller sees a [CallInvalidInput] failure.
func InvalidInput(format string, args ...any) error {
	return &CallError{Kind: CallInvalidInput, Message: fmt.Sprintf(format, args...)}
}

// ConstraintViolation rejects a tool invocation that breaks a rule of the
// command or of a middleware's policy; the caller sees a
// [CallConstraintViolation] failure.
func ConstraintViolation(format string, args ...any) error {
	return &CallError{Kind: CallConstraintViolation, Message: fmt.Sprintf(format, args...)}
}

func (e *CallError) Error() string {
	msg := "golem: " + e.Kind.String()
	if e.Tool != "" {
		msg = fmt.Sprintf("golem: tool %s %s: %s", e.Tool, commandLabel(e.CommandPath), e.Kind)
	}
	if e.ErrorName != "" {
		msg += " " + e.ErrorName
	}
	if e.Message != "" {
		msg += ": " + e.Message
	}
	return msg
}

// toolCallErrorFromWit classifies the host's tool RPC failure.
func toolCallErrorFromWit(tool string, path []string, e types.ToolRpcError) *CallError {
	out := &CallError{Tool: tool, CommandPath: slices.Clone(path)}
	switch e.Tag() {
	case types.ToolRpcErrorProtocolError:
		out.Kind, out.Message = CallProtocolError, e.ProtocolError()
	case types.ToolRpcErrorDenied:
		out.Kind, out.Message = CallDenied, e.Denied()
	case types.ToolRpcErrorNotFound:
		out.Kind, out.Message = CallUnknownTool, e.NotFound()
	case types.ToolRpcErrorRemoteInternalError:
		out.Kind, out.Message = CallInternalError, e.RemoteInternalError()
	case types.ToolRpcErrorCancelled:
		out.Kind = CallCancelled
	case types.ToolRpcErrorResourceExhausted:
		out.Kind, out.Message = CallResourceExhausted, e.ResourceExhausted()
	case types.ToolRpcErrorRemoteToolError:
		te := e.RemoteToolError()
		out.wire = &te
		switch te.Tag() {
		case types.ToolErrorInvalidToolName:
			out.Kind, out.Message = CallUnknownTool, te.InvalidToolName()
		case types.ToolErrorInvalidCommandPath:
			out.Kind = CallUnknownCommand
		case types.ToolErrorInvalidInput:
			out.Kind, out.Message = CallInvalidInput, te.InvalidInput()
		case types.ToolErrorConstraintViolation:
			out.Kind, out.Message = CallConstraintViolation, te.ConstraintViolation()
		case types.ToolErrorInvalidResult:
			out.Kind, out.Message = CallInvalidResult, te.InvalidResult()
		case types.ToolErrorCustomError:
			ce := te.CustomError()
			out.Kind, out.ErrorName, out.payload = CallDeclaredError, ce.Name, typedValue(ce.Payload)
		default:
			out.Kind = CallInvalidResult
		}
	default:
		out.Kind = CallInternalError
	}
	return out
}

// toolCall is a started call as the host hands it back: the outputs that were
// requested, and the pending outcome.
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
func (c *Command[T, A, O]) Call(fill func(*A)) (O, error) {
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
func (c *Command[T, A, O]) On(client *Client[T]) *Command[T, A, O] {
	return &Command[T, A, O]{ce: c.ce, target: client.name}
}

// Call starts the command and returns the running invocation, whose outputs
// are read while the command runs. Every output the command declares is
// requested. fill is as for [Command.Call].
func (c *OutputCommand[T, A, O]) Call(fill func(*A)) (*Invocation[O], error) {
	target := c.ce.targetName(c.target)
	call, err := c.ce.start(target, fillArgs(fill))
	if err != nil {
		return nil, err
	}
	return commandInvocation[O](c.ce, target, call), nil
}

// On targets the command at the tool registered under the client's name,
// instead of the definition's own.
func (c *OutputCommand[T, A, O]) On(client *Client[T]) *OutputCommand[T, A, O] {
	return &OutputCommand[T, A, O]{ce: c.ce, target: client.name}
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

func (ce *commandEntry) callError(target string, kind CallErrorKind, format string, args ...any) *CallError {
	return &CallError{
		Tool: target, CommandPath: slices.Clone(ce.node.path),
		Kind: kind, Message: fmt.Sprintf(format, args...),
	}
}

func (ce *commandEntry) start(target string, fill func(reflect.Value)) (toolCall, error) {
	input, stdin, err := ce.prepare(target, fill)
	if err != nil {
		return toolCall{}, err
	}
	return startToolCall(target, ce.node.path, input, stdin, ce.streams())
}

// streams are the outputs a call of the command requests: every declared one.
func (ce *commandEntry) streams() Streams {
	st := ce.spec.settings
	return Streams{Stdout: st.stdout != nil, Stderr: st.stderr != nil}
}

// Streams selects the outputs a call requests.
type Streams struct{ Stdout, Stderr bool }

// prepare builds a call's input: the declared defaults, then fill, rendered
// as the canonical input record, and the standard input field.
func (ce *commandEntry) prepare(target string, fill func(reflect.Value)) (types.TypedSchemaValue, io.Reader, error) {
	e := ce.node.entry
	l, ok := ce.resolve()
	if !ok || ce.node.body != ce {
		return types.TypedSchemaValue{}, nil, fmt.Errorf("golem: tool %s command %s is not well-defined:\n%s",
			e.name, ce.label(), engine.AllErrors(e.d.Errs))
	}
	args := reflect.New(ce.argsType).Elem()
	args.Set(l.defaults)
	fill(args)

	var stdin io.Reader
	if l.stdin != nil {
		stdin, _ = args.FieldByIndex(l.stdin.path).Interface().(io.Reader)
		if stdin == nil && !l.stdin.optional {
			return types.TypedSchemaValue{}, nil, ce.callError(target, CallInvalidInput, "the command requires standard input")
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
	if ce.outType == reflect.TypeFor[golem.Unit]() {
		return out, nil
	}
	if res.IsNone() {
		return reflect.Value{}, ce.callError(target, CallInvalidResult, "the command returned no result")
	}
	tree := res.Some().Value
	dec := engine.Decoder{Nodes: tree.ValueNodes}
	if err := ce.node.entry.d.Compile(ce.outType).Decode(&dec, out, tree.Root); err != nil {
		return reflect.Value{}, ce.callError(target, CallInvalidResult, "%v", err)
	}
	return out, nil
}

// Invocation is a running call of a command with outputs. Take an output
// with Stdout or Stderr and read it while the command runs, then Wait for the
// result; or Collect everything.
//
// Wait drains, and discards, every output that was not taken, so a command
// writing more than the stream buffers never stalls on output nobody reads.
// An output taken is the caller's to read: Wait leaves it alone, and if it is
// not read concurrently a command writing more than the buffers hold stalls.
//
// Beneath a middleware, an output the middleware does not take is passed
// through to its own output of the same name instead of being discarded.
type Invocation[O any] struct {
	call   toolCall
	finish func(toolCall) (O, error)
	stdout invocationOutput
	stderr invocationOutput
	// passThrough is set for a call beneath a middleware, whose untaken outputs
	// go to the middleware's own.
	passThrough *middlewareInvocation
	done        bool
	out         O
	err         error
}

// invocationOutput is one output of a running call and who has it.
type invocationOutput struct {
	r       *byteReader
	taken   bool
	drained bool
}

func (o *invocationOutput) take() io.Reader {
	switch {
	case o.drained:
		return errReader{ErrOutputDrained}
	case o.r == nil:
		return strings.NewReader("")
	}
	o.taken = true
	return o.r
}

type errReader struct{ err error }

func (r errReader) Read([]byte) (int, error) { return 0, r.err }

// commandInvocation is a running call of a declared command, whose result is
// decoded into the command's result type.
func commandInvocation[O any](ce *commandEntry, target string, call toolCall) *Invocation[O] {
	return newInvocation(call, func(c toolCall) (O, error) {
		var zero O
		out, err := ce.finish(target, c)
		if err != nil {
			return zero, err
		}
		o, _ := out.Interface().(O)
		return o, nil
	})
}

func newInvocation[O any](call toolCall, finish func(toolCall) (O, error)) *Invocation[O] {
	return &Invocation[O]{
		call: call, finish: finish,
		stdout: invocationOutput{r: call.stdout}, stderr: invocationOutput{r: call.stderr},
	}
}

// Stdout takes the command's standard output. It ends with io.EOF when the
// command finishes it and with a [OutputError] when the command fails it; it
// is empty for a command without one, and fails with [ErrOutputDrained] after
// Wait drained it.
func (i *Invocation[O]) Stdout() io.Reader { return i.stdout.take() }

// Stderr takes the command's standard error, like Stdout. Bytes on it do not
// mean the command failed.
func (i *Invocation[O]) Stderr() io.Reader { return i.stderr.take() }

// Wait awaits the command's result, draining every output that was not taken
// meanwhile (see [Invocation]).
func (i *Invocation[O]) Wait() (O, error) {
	if i.done {
		return i.out, i.err
	}
	i.done = true
	var relays []chan error
	for _, o := range []struct {
		out *invocationOutput
		dst func(*middlewareInvocation) *Output
	}{
		{&i.stdout, func(m *middlewareInvocation) *Output { return m.stdout }},
		{&i.stderr, func(m *middlewareInvocation) *Output { return m.stderr }},
	} {
		if o.out.taken || o.out.drained || o.out.r == nil {
			continue
		}
		o.out.drained = true
		var dst *Output
		if i.passThrough != nil {
			dst = o.dst(i.passThrough)
		}
		done := make(chan error, 1)
		relays = append(relays, done)
		go func(src *byteReader) { done <- passOn(dst, src, i.call.cancel) }(o.out.r)
	}
	i.out, i.err = i.finish(i.call)
	for _, done := range relays {
		// A failure writing a middleware's own output cancelled the call
		// beneath, and is why it failed.
		if err := <-done; err != nil {
			var zero O
			i.out, i.err = zero, err
		}
	}
	return i.out, i.err
}

// passOn copies an output of a call into dst, or discards it when there is no
// dst or the command dst belongs to does not declare it. A failure of the
// output itself is passed on as dst's terminal; a failure writing dst cancels
// the call and is returned.
func passOn(dst *Output, src *byteReader, cancel func()) error {
	if dst == nil || dst.undeclared != "" {
		_, _ = io.Copy(io.Discard, src)
		return nil
	}
	buf := make([]byte, 32*1024)
	for {
		n, err := src.Read(buf)
		if n > 0 {
			if _, werr := dst.Write(buf[:n]); werr != nil {
				src.close()
				cancel()
				return werr
			}
		}
		var se *OutputError
		switch {
		case err == nil:
		case errors.Is(err, io.EOF):
			return nil
		case errors.As(err, &se):
			dst.fail(se.Failure)
			return nil
		default:
			dst.fail(OutputFailed(err.Error()))
			return nil
		}
	}
}

// Cancel asks the runtime to cancel the call.
func (i *Invocation[O]) Cancel() { i.call.cancel() }

// Collected is everything a call produced.
type Collected[O any] struct {
	Result O
	Stdout []byte
	Stderr []byte
}

// Collect reads every output to its end while awaiting the result. A failed
// result is reported before a failed standard output, and that before a
// failed standard error; what was read is returned either way.
func (i *Invocation[O]) Collect() (Collected[O], error) {
	var c Collected[O]
	read := func(r io.Reader, into *[]byte) chan error {
		done := make(chan error, 1)
		go func() {
			data, err := io.ReadAll(r)
			*into = data
			done <- err
		}()
		return done
	}
	stdout := read(i.Stdout(), &c.Stdout)
	stderr := read(i.Stderr(), &c.Stderr)
	result, err := i.Wait()
	c.Result = result
	stdoutErr, stderrErr := <-stdout, <-stderr
	switch {
	case err != nil:
		return c, err
	case stdoutErr != nil:
		return c, stdoutErr
	default:
		return c, stderrErr
	}
}
