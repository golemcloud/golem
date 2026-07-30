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

// SDK-owned counterparts of the WIT types that Go has no native spelling for.
//
// Their fields are unexported and they are built through constructors, so an
// inconsistent value (an "ok" result carrying an error, say) cannot be
// constructed. The codec reaches into them through the unexported interfaces
// below rather than by field reflection — which is also why these cannot be the
// wit-bindgen types: those have unexported fields and no accessors, so
// reflection can neither read nor write them.

// ---------------------------------------------------------------------------
// Option
// ---------------------------------------------------------------------------

// Option is an explicit optional value.
//
// A plain *T means option<T> too, and is the idiomatic spelling for a struct
// field. Reach for Option[T] when a pointer would be ambiguous or awkward —
// most often when nesting, where Option[Option[T]] reads far better than **T.
// Both produce exactly the same schema.
type Option[T any] struct {
	some  bool
	value T
}

// Some returns an Option holding v.
func Some[T any](v T) Option[T] { return Option[T]{some: true, value: v} }

// None returns an empty Option.
func None[T any]() Option[T] { return Option[T]{} }

// IsSome reports whether a value is present.
func (o Option[T]) IsSome() bool { return o.some }

// IsNone reports whether the Option is empty.
func (o Option[T]) IsNone() bool { return !o.some }

// Get returns the value and whether it was present — the comma-ok form.
func (o Option[T]) Get() (T, bool) { return o.value, o.some }

// Or returns the value if present, otherwise def.
func (o Option[T]) Or(def T) T {
	if o.some {
		return o.value
	}
	return def
}

// Unwrap returns the value, panicking if the Option is empty. Use Get when
// absence is expected.
func (o Option[T]) Unwrap() T {
	if !o.some {
		panic("golem: Unwrap on an empty Option")
	}
	return o.value
}

// optionish is the codec plumbing for Option, resolved once at compile time
// against the zero value, so there is no per-value type switching and no unsafe
// field access.
type optionish interface {
	optionElem() reflect.Type
	optionGet() (reflect.Value, bool)
}

type optionSetter interface {
	optionSetNone()
	optionSetSome() reflect.Value
}

func (o Option[T]) optionElem() reflect.Type { return reflect.TypeFor[T]() }

func (o Option[T]) optionGet() (reflect.Value, bool) {
	return reflect.ValueOf(&o.value).Elem(), o.some
}

func (o *Option[T]) optionSetNone() {
	var zero T
	o.some, o.value = false, zero
}

func (o *Option[T]) optionSetSome() reflect.Value {
	o.some = true
	return reflect.ValueOf(&o.value).Elem()
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

// Result is a value that is either a success or a typed failure.
//
// It is not how a method reports failure — a handler panics for that, which
// surfaces as the WIT agent-error channel. Result is for a fallible value
// delivered on a *successful* invocation: a return value the caller inspects, a
// field, a list element, one arm of another Result. That distinction mirrors the
// TS SDK, where a Result lowers to a component-model result inside the success
// payload while throws stay on the agent-error channel.
type Result[Ok any, Err any] struct {
	isErr bool
	ok    Ok
	err   Err
}

// Ok returns a successful Result. Both type parameters are explicit because
// neither can be inferred from the argument alone:
//
//	golem.Ok[Money, string](m)
func Ok[O any, E any](v O) Result[O, E] { return Result[O, E]{ok: v} }

// Err returns a failed Result.
func Err[O any, E any](e E) Result[O, E] { return Result[O, E]{isErr: true, err: e} }

// IsOk reports whether the Result is a success.
func (r Result[Ok, Err]) IsOk() bool { return !r.isErr }

// IsErr reports whether the Result is a failure.
func (r Result[Ok, Err]) IsErr() bool { return r.isErr }

// Ok returns the typed success value, panicking (with the Err payload) if the
// Result is a failure. Check IsOk first, use the typed [Result.Err], or use
// [Result.Get] to bridge to (value, error).
func (r Result[Ok, Err]) Ok() Ok {
	if r.isErr {
		panic(fmt.Errorf("golem: Result.Ok() on a failed Result: %v", r.err))
	}
	return r.ok
}

// Err returns the typed failure value, panicking if the Result is a success.
func (r Result[Ok, Err]) Err() Err {
	if !r.isErr {
		panic("golem: Result.Err() on a successful Result")
	}
	return r.err
}

// Get bridges the Result to idiomatic Go (value, error): it returns the success
// value and a nil error, or the zero value and an error carrying the Err arm. A
// data-typed Err (string, struct, enum) is wrapped in a [ResultError] that keeps
// the typed payload recoverable via [errors.As]; an Err that already implements
// error is returned as-is. Compose with [Must] to treat the failure as fatal:
//
//	total, err := ledger.Record.Call(l, in).Get()  // handle
//	total := golem.Must(ledger.Record.Call(l, in).Get())  // or fail-loud
//
// Use the typed [Result.Ok]/[Result.Err] instead when you want the Err value in
// its own type rather than as an error.
func (r Result[Ok, Err]) Get() (Ok, error) {
	if r.isErr {
		var zero Ok
		return zero, asGoError(r.err)
	}
	return r.ok, nil
}

// OkOr returns the success value if present, otherwise def.
func (r Result[Ok, Err]) OkOr(def Ok) Ok {
	if r.isErr {
		return def
	}
	return r.ok
}

// ResultError adapts a data-typed Err arm (one that is not itself an error) into
// a Go error, so [Result.Get] can bridge to (value, error) without flattening the
// typed payload — recover it with [errors.As]:
//
//	_, err := res.Get()
//	var re *golem.ResultError[string]
//	if errors.As(err, &re) { use(re.Value) }
type ResultError[E any] struct{ Value E }

func (e *ResultError[E]) Error() string { return fmt.Sprintf("%v", e.Value) }

// asGoError turns an Err-arm value into a Go error: the value itself if it already
// implements error, otherwise a ResultError wrapper preserving the typed payload.
func asGoError[E any](e E) error {
	if err, ok := any(e).(error); ok {
		return err
	}
	return &ResultError[E]{Value: e}
}

type resultish interface {
	resultElems() (ok, err reflect.Type)
	resultGet() (v reflect.Value, isErr bool)
}

type resultSetter interface {
	resultSetOk() reflect.Value
	resultSetErr() reflect.Value
}

func (r Result[Ok, Err]) resultElems() (reflect.Type, reflect.Type) {
	return reflect.TypeFor[Ok](), reflect.TypeFor[Err]()
}

func (r Result[Ok, Err]) resultGet() (reflect.Value, bool) {
	if r.isErr {
		return reflect.ValueOf(&r.err).Elem(), true
	}
	return reflect.ValueOf(&r.ok).Elem(), false
}

func (r *Result[Ok, Err]) resultSetOk() reflect.Value {
	r.isErr = false
	return reflect.ValueOf(&r.ok).Elem()
}

func (r *Result[Ok, Err]) resultSetErr() reflect.Value {
	r.isErr = true
	return reflect.ValueOf(&r.err).Elem()
}

// ---------------------------------------------------------------------------
// Markers
// ---------------------------------------------------------------------------

// Char is a single Unicode code point, lowering to the WIT char type. Go's rune
// is an alias for int32 and so is indistinguishable from a plain integer; this
// named type is what makes the intent visible to the deriver.
type Char rune

// URL is a string constrained to a URL, lowering to the WIT url type.
type URL string

// Secret is a handle to a declared config secret, obtained from the agent's
// config ([Agent.Config] / [InitContext.Config]). It lowers to the WIT secret
// type. [Secret.Get] reads the CURRENT plaintext from the host on each call, so a
// rotated value is observed; the payload stays redacted in logs. A Secret cannot
// be constructed from a plaintext and cannot be a method parameter or return
// value — it is config-only.
type Secret[T any] struct {
	// read fetches the current value from the host. It is installed by config
	// materialization (see secretBindPath); a zero-value Secret has none.
	read func() (T, error)
}

// Get reads the secret's current plaintext from the host and returns it, or
// panics if the read fails. Because it re-reads on every call, a rotated secret
// is observed rather than a stale snapshot. A secret read failing is a hard
// failure with no in-band recovery, so — like the TS/Rust/Scala SDKs — Get fails
// loud: the panic is recovered by the invoke dispatcher into an agent-error. Must
// be called inside an invocation (it calls the host).
func (s Secret[T]) Get() T {
	if s.read == nil {
		// Only reachable for a zero-value Secret — one never obtained from config
		// (e.g. `var s Secret[string]`). That is a programming mistake, not an
		// operational state; Go cannot forbid the zero value of an exported struct,
		// so this guard turns the otherwise-cryptic nil-closure panic into a clear
		// message.
		panic("golem: Secret has no source; obtain it from the agent's config")
	}
	v, err := s.read()
	if err != nil {
		panic(err)
	}
	return v
}

// String keeps secrets out of logs and error messages formatted with %v or %s.
func (s Secret[T]) String() string { return "golem.Secret(redacted)" }

// GoString does the same for %#v.
func (s Secret[T]) GoString() string { return "golem.Secret(redacted)" }

// secretish identifies a Secret[T] by its inner type — used for config-leaf
// detection ([isSecretType]) and for building the secret(inner) schema node.
type secretish interface{ secretElem() reflect.Type }

// secretBinder installs the live-read closure during config materialization. It
// is driven by reflection ([readSecretLeaf]) because the inner T is not
// statically known there.
type secretBinder interface{ secretBindPath(path []string) }

func (s Secret[T]) secretElem() reflect.Type { return reflect.TypeFor[T]() }

func (s *Secret[T]) secretBindPath(path []string) {
	s.read = func() (T, error) { return readSecretValue[T](defs, path) }
}

// ---------------------------------------------------------------------------
// (value, error) bridges
// ---------------------------------------------------------------------------

// Must adapts an ordinary Go (value, error) call into the panic-as-failure
// model: it returns the value, or panics if err is non-nil. Go's multi-value
// passing lets a call be dropped straight in:
//
//	n := golem.Must(strconv.Atoi(ctx.State.raw))
//
// Use it for failures that should abort the invocation. For an expected outcome
// the caller should inspect, produce a [Result] instead (see [ResultOf]).
func Must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

// ResultOf turns a Go (value, error) pair into a Result value, so a fallible
// call can be surfaced to the caller as data rather than aborting:
//
//	return golem.ResultOf(strconv.Atoi(s))   // Result[int, string]
//
// The error becomes the Err arm as its message string (Go's error is an
// interface with no wire schema; the string is what crosses the boundary).
func ResultOf[T any](v T, err error) Result[T, string] {
	if err != nil {
		return Err[T, string](err.Error())
	}
	return Ok[T, string](v)
}

// Must2 is Must for the common (value, extra, error) shape — e.g. Go API
// clients that return (result, *http.Response, error). It returns the first two
// values, or panics if err is non-nil:
//
//	repo, _ := golem.Must2(gh.Repositories.Get(ctx, "owner", "name"))
func Must2[A any, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

// Must0 is Must for a call that returns only an error: it panics if err is
// non-nil and otherwise does nothing. Use it to abort the invocation on a
// fallible, side-effecting call that yields no value:
//
//	golem.Must0(bucket.Set(key, value))
func Must0(err error) {
	if err != nil {
		panic(err)
	}
}
