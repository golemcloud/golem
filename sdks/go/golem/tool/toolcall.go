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
	"github.com/golemcloud/golem/sdks/go/golem/internal/link"
	"io"
	"reflect"
	"slices"
	"strings"
	"sync"

	"github.com/golemcloud/golem/sdks/go/golem/internal/observer"
	"github.com/golemcloud/golem/sdks/go/golem/internal/witschema"

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
// A command declared with OutputCommand returns an [Invocation] instead,
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
	// ErrorName is the declared error case, for [CallDeclaredError].
	ErrorName string
	payload   golem.TypedValue
	// decoded holds the payload as each successful Match decoded it, by Go
	// type. Decoding moves a permission card out of the payload, so every
	// later Match for that type, and every copy of the error, reads it from
	// here.
	decoded *decodedPayloads
	// wire is the tool's own error, kept so a middleware passing the failure
	// on returns it unchanged.
	wire *types.ToolError
}

// Payload returns a declared error's payload; read it with JSON, or decode it
// with [DecodeTypedValue]. A typed caller matches the case with its Match.
//
// A permission card in the payload moves out with the first decode, either
// Match's or DecodeTypedValue's; a later DecodeTypedValue of the payload
// fails for it. Match decodes once and returns the same value every time.
func (e *CallError) Payload() golem.TypedValue { return e.payload }

// decodedPayloads is a declared error's payload as each Go type it decoded
// as.
type decodedPayloads struct{ byType map[reflect.Type]any }

// decodePayload decodes the payload as P, and keeps a successful decode for
// every later call for P. A failed decode leaves the payload as it was, so it
// is not kept: a later call decodes again. A decode as another type after a
// successful one fails for a card the first one took.
func decodePayload[P any](e *CallError) (P, error) {
	typ := reflect.TypeFor[P]()
	if e.decoded == nil {
		e.decoded = &decodedPayloads{}
	}
	if v, ok := e.decoded.byType[typ]; ok {
		p, _ := v.(P)
		return p, nil
	}
	p, err := golem.DecodeTypedValue[P](e.payload)
	if err == nil {
		if e.decoded.byType == nil {
			e.decoded.byType = map[reflect.Type]any{}
		}
		e.decoded.byType[typ] = p
	}
	return p, err
}

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
			out.decoded = &decodedPayloads{}
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
	drop   func()
}

type toolOutcome struct {
	result   witTypes.Option[types.TypedSchemaValue]
	err      *types.ToolRpcError
	relayErr error
}

func (r toolOutcome) dispose() {
	if r.result.IsSome() {
		witschema.ReleaseTree(r.result.Some().Value)
	}
	if r.err != nil && r.err.Tag() == types.ToolRpcErrorRemoteToolError {
		e := r.err.RemoteToolError()
		if e.Tag() == types.ToolErrorCustomError {
			witschema.ReleaseTree(e.CustomError().Payload.Value)
		}
	}
}

// ErrResultDetached means the local result observer no longer delivers results.
var ErrResultDetached = observer.ErrDetached

// guardCancel makes cancel a no-op once wait has returned: wait releases the
// host future that cancel addresses, so a late cancel (a deferred Cancel, or a
// relay that fails after the result arrived) must not reach it.
// Cancel and drop are synchronous host calls and must not re-enter this guard.
func guardCancel(c toolCall) toolCall {
	var mu sync.Mutex
	settled := false
	wait, cancel, drop := c.wait, c.cancel, c.drop
	c.drop = func() {
		mu.Lock()
		defer mu.Unlock()
		if !settled {
			settled = true
			if drop != nil {
				drop()
			}
		}
	}
	c.wait = func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) {
		defer func() {
			r := recover()
			func() {
				defer func() {
					if dropPanic := recover(); r == nil {
						r = dropPanic
					}
				}()
				c.drop()
			}()
			if r != nil {
				panic(r)
			}
		}()
		return wait()
	}
	c.cancel = func() {
		mu.Lock()
		defer mu.Unlock()
		if !settled && cancel != nil {
			cancel()
		}
	}
	return c
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
	input, stdin, restore, err := ce.prepare(target, fill)
	if err != nil {
		return toolCall{}, err
	}
	call, err := startToolCall(target, ce.node.path, input, stdin, ce.streams())
	if err != nil {
		// The host took nothing: the cards in the arguments go back.
		restore()
	}
	return call, err
}

// streams are the outputs a call of the command requests: every declared one.
func (ce *commandEntry) streams() Streams {
	st := ce.spec.settings
	return Streams{Stdout: st.stdout != nil, Stderr: st.stderr != nil}
}

// Streams selects the outputs a call requests.
type Streams struct{ Stdout, Stderr bool }

// prepare builds a call's input: the declared defaults, then fill, rendered
// as the canonical input record, and the standard input field. restore gives
// the cards the input moved back, for a call that fails to start.
func (ce *commandEntry) prepare(target string, fill func(reflect.Value)) (input types.TypedSchemaValue, stdin io.Reader, restore func(), err error) {
	e := ce.node.entry
	l, ok := ce.resolve()
	if !ok || ce.node.body != ce {
		return types.TypedSchemaValue{}, nil, nil, fmt.Errorf("golem: tool %s command %s is not well-defined:\n%s",
			e.name, ce.label(), engine.AllErrors(e.d.Errs))
	}
	args := reflect.New(ce.argsType).Elem()
	args.Set(l.defaults)
	for _, a := range l.fields {
		if a.b.def.IsValid() {
			args.FieldByIndex(a.path).Set(cloneValue(e.d, a.b.def))
		}
	}
	fill(args)

	if l.stdin != nil {
		stdin, _ = args.FieldByIndex(l.stdin.path).Interface().(io.Reader)
		if stdin == nil && !l.stdin.optional {
			return types.TypedSchemaValue{}, nil, nil, ce.callError(target, CallInvalidInput, "the command requires standard input")
		}
	}
	input, restore = l.encode(e.d, args)
	return input, stdin, restore, nil
}

// finish awaits the outcome and decodes the result.
func (ce *commandEntry) finish(target string, call toolCall) (reflect.Value, error) {
	res, rpcErr := call.wait()
	if rpcErr != nil {
		return reflect.Value{}, toolCallErrorFromWit(target, ce.node.path, *rpcErr)
	}
	if res.IsNone() {
		if ce.outType == reflect.TypeFor[golem.Unit]() {
			return reflect.New(ce.outType).Elem(), nil
		}
		return reflect.Value{}, ce.callError(target, CallInvalidResult, "the command returned no result")
	}
	tree := res.Some().Value
	return witschema.DecodeOwned(tree, func() (reflect.Value, error) {
		if ce.outType == reflect.TypeFor[golem.Unit]() {
			return reflect.Value{}, ce.callError(target, CallInvalidResult, "the command returned a value but declares none")
		}
		out := reflect.New(ce.outType).Elem()
		dec := engine.Decoder{Nodes: tree.ValueNodes}
		if err := ce.node.entry.d.Compile(ce.outType).Decode(&dec, out, tree.Root); err != nil {
			// The adopted cards go back into the tree, which releases them with
			// the rest of its capabilities.
			dec.Restore()
			return reflect.Value{}, ce.callError(target, CallInvalidResult, "%v", err)
		}
		dec.ReleaseRest(link.ReleaseCard)
		return out, nil
	})
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
	mu     sync.Mutex
	result observer.Observer[toolOutcome, O]
	call   toolCall
	stdout invocationOutput
	stderr invocationOutput
	// passThrough is set for a call beneath a middleware, whose untaken outputs
	// go to the middleware's own.
	passThrough *middlewareInvocation
}

// invocationOutput is one output of a running call and who has it.
type invocationOutput struct {
	r       *byteReader
	taken   bool
	drained bool
}

func (o *invocationOutput) take() io.ReadCloser {
	switch {
	case o.drained:
		return io.NopCloser(errReader{ErrOutputDrained})
	case o.r == nil:
		return io.NopCloser(strings.NewReader(""))
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
	i := &Invocation[O]{
		call:   call,
		stdout: invocationOutput{r: call.stdout}, stderr: invocationOutput{r: call.stderr},
	}
	i.result.GetRaw = i.awaitRaw
	i.result.Decode = func(r toolOutcome) (O, error) {
		if r.relayErr != nil {
			r.dispose()
			var zero O
			return zero, r.relayErr
		}
		c := call
		c.wait = func() (witTypes.Option[types.TypedSchemaValue], *types.ToolRpcError) { return r.result, r.err }
		return finish(c)
	}
	i.result.Dispose = func(r toolOutcome) { r.dispose() }
	i.result.Drop, i.result.CancelRaw = call.drop, call.cancel
	return i
}

// Stdout takes the command's standard output. It ends with io.EOF when the
// command finishes it and with a [OutputError] when the command fails it; it
// is empty for a command without one, and fails with [ErrOutputDrained] after
// Wait drained it. Close stops reading it: the command sees the output closed
// by its consumer, and the call and its other output go on.
func (i *Invocation[O]) Stdout() io.ReadCloser {
	i.mu.Lock()
	defer i.mu.Unlock()
	return i.stdout.take()
}

// Stderr takes the command's standard error, like Stdout. Bytes on it do not
// mean the command failed.
func (i *Invocation[O]) Stderr() io.ReadCloser {
	i.mu.Lock()
	defer i.mu.Unlock()
	return i.stderr.take()
}

// Wait awaits the command's result, draining every output that was not taken
// meanwhile (see [Invocation]). If result delivery is detached, Wait starts any
// untaken output drains but returns ErrResultDetached without awaiting them.
func (i *Invocation[O]) Wait() (O, error) {
	out, err := i.result.Get()
	if errors.Is(err, ErrResultDetached) {
		i.startOutputRelays()
	}
	return out, err
}

func (i *Invocation[O]) startOutputRelays() []chan error {
	i.mu.Lock()
	defer i.mu.Unlock()
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
		go func(src *byteReader) { done <- passOn(dst, src) }(o.out.r)
	}
	return relays
}

func (i *Invocation[O]) awaitRaw() toolOutcome {
	relays := i.startOutputRelays()
	r, e := i.call.wait()
	outcome := toolOutcome{result: r, err: e}
	detached := i.result.Detached()
	for _, done := range relays {
		// Relay failures are SDK errors, not ordinary handler errors.
		select {
		case err := <-done:
			if err != nil {
				outcome.relayErr = err
			}
		case <-detached:
			// The raw result can release now; the output relays continue.
			return outcome
		}
	}
	return outcome
}

// Detach stops result delivery without cancelling the invocation or closing
// either output. An active host Get settles before its resource is dropped.
// Detach alone does not drain outputs: call Wait to start draining untaken
// outputs, or take and read or close each output. Otherwise an unread output
// can retain its host resource and block the producer on backpressure.
func (i *Invocation[O]) Detach() { i.result.Detach() }

// passOn copies an output of a call into dst, or discards it when there is no
// dst or the command dst belongs to does not declare it. A failure of the
// output itself is passed on as dst's terminal. A closed destination closes
// only this source; other write failures are returned as typed SDK errors.
func passOn(dst *Output, src *byteReader) error {
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
				var closed *outputClosedError
				var failed *OutputError
				if errors.As(werr, &closed) || errors.As(werr, &failed) {
					return nil
				}
				return &CallError{Kind: CallInvalidResult, Message: werr.Error()}
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
func (i *Invocation[O]) Cancel() { i.result.Cancel() }

// Collected is everything a call produced. Each part carries its own outcome,
// so a failed output does not hide the result, and a failed result does not
// hide what the outputs carried.
type Collected[O any] struct {
	// Result is the command's result, valid when Err is nil.
	Result O
	// Err is why the call failed, usually a [*CallError].
	Err error
	// Stdout is what the command wrote to its standard output: nil for a
	// command without one, and what was read before a failure otherwise.
	Stdout []byte
	// StdoutErr is why the standard output did not end normally, usually an
	// [*OutputError] carrying the failure the command reported.
	StdoutErr error
	// Stderr is what the command wrote to its standard error, like Stdout.
	Stderr []byte
	// StderrErr is why the standard error did not end normally.
	StderrErr error
}

// Collect reads every output to its end while awaiting the result.
func (i *Invocation[O]) Collect() Collected[O] {
	var c Collected[O]
	read := func(o *invocationOutput, into *[]byte, failed *error) chan struct{} {
		done := make(chan struct{})
		i.mu.Lock()
		declared := o.r != nil || o.drained
		r := o.take()
		i.mu.Unlock()
		go func() {
			defer close(done)
			data, err := io.ReadAll(r)
			if declared {
				*into = data
			}
			*failed = err
		}()
		return done
	}
	stdout := read(&i.stdout, &c.Stdout, &c.StdoutErr)
	stderr := read(&i.stderr, &c.Stderr, &c.StderrErr)
	c.Result, c.Err = i.Wait()
	<-stdout
	<-stderr
	return c
}

// cloneValue deep-copies a schema-valued v through its codec, so a call that
// edits a map or slice default does not change the declared default.
func cloneValue(d *definitions, v reflect.Value) reflect.Value {
	c := d.Compile(v.Type())
	tree := engine.EncodeWith(c, v)
	out := reflect.New(v.Type()).Elem()
	dec := engine.Decoder{Nodes: tree.ValueNodes}
	if err := c.Decode(&dec, out, tree.Root); err != nil {
		panic(fmt.Errorf("golem: copying the default %s: %w", v.Type(), err))
	}
	return out
}
