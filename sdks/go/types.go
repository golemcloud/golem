// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import "reflect"

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

// Codec plumbing. Resolved once at compile time against the zero value, so
// there is no per-value type switching and no unsafe field access.
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
// It is not how a method reports failure — a handler returns (Out, error), and
// a returned error becomes the WIT agent-error channel. Result is for a
// fallible value nested *inside* data: a field, a list element, one arm of
// another Result. That distinction mirrors the TS SDK, where a Result lowers to
// a component-model result inside the success payload while throws stay on the
// agent-error channel.
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

// Ok returns the success value, panicking if the Result is a failure. Check
// IsOk first, or use Get.
func (r Result[Ok, Err]) Ok() Ok {
	if r.isErr {
		panic("golem: Ok on a failed Result")
	}
	return r.ok
}

// Err returns the failure value, panicking if the Result is a success.
func (r Result[Ok, Err]) Err() Err {
	if !r.isErr {
		panic("golem: Err on a successful Result")
	}
	return r.err
}

// Get returns the success value and whether the Result succeeded.
func (r Result[Ok, Err]) Get() (Ok, bool) { return r.ok, !r.isErr }

// OkOr returns the success value if present, otherwise def.
func (r Result[Ok, Err]) OkOr(def Ok) Ok {
	if r.isErr {
		return def
	}
	return r.ok
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
