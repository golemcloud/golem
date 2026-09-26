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

// The Go vocabulary for schema values.
//
// These are the types Go has no native spelling for — an explicit option, a
// result, a tuple, prose as distinct from an identifier, a measurement carrying
// its unit. They live here, in the shared module, so a guest agent and an
// external client generated from the same schema get the *same* Go types: a
// program can hold one domain package and use it on both sides.
//
// Their fields are unexported and they are built through constructors, so an
// inconsistent value — an "ok" result carrying an error, say — cannot be
// constructed. A reflective codec reaches into them through the sealed
// interfaces below rather than by field reflection. The interfaces are exported
// so a codec in another module can assert against them; their methods are not,
// so only the types here can implement them.

package values

import (
	"fmt"
	"reflect"
)

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
		panic(fmt.Errorf("golem: Unwrap on an empty Option"))
	}
	return o.value
}

// Optionish lets a codec read an Option without knowing T. It is resolved once
// at compile time against the zero value, so there is no per-value type
// switching and no unsafe field access. The unexported methods seal it: another
// module can assert against it, but only Option can implement it.
type Optionish interface {
	optionElem() reflect.Type
	optionGet() (reflect.Value, bool)
}

type OptionSetter interface {
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
// surfaces as the agent-error channel. Result is for a fallible value delivered
// on a *successful* invocation: a return value the caller inspects, a field, a
// list element, or one arm of another Result.
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
		panic(fmt.Errorf("golem: Result.Err() on a successful Result"))
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

// MustOk returns the success value, or panics with the failure as a Go error if
// the Result is a failure. It is the fail-loud shorthand for the common
// "propagate the failure" case and is equivalent to golem.Must(r.Get()):
//
//	total := ledger.Record.Call(l, in).MustOk()
//
// The panic carries the Err arm as an error — a data-typed Err is wrapped in a
// [ResultError] recoverable via errors.As, matching [Result.Get]. Use the typed
// [Result.Ok]/[Result.Err] when you want the Err value in its own type.
func (r Result[Ok, Err]) MustOk() Ok {
	if r.isErr {
		panic(asGoError(r.err))
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

type Resultish interface {
	resultElems() (ok, err reflect.Type)
	resultGet() (v reflect.Value, isErr bool)
}

type ResultSetter interface {
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

// Text is human-language prose, lowering to the WIT text type. A plain string
// lowers to string; this named type is what marks the value as natural language
// so a reader (or a model) is told it is prose rather than an identifier.
type Text string

// Binary is an opaque byte payload, lowering to the WIT binary type. A plain
// []byte stays a list<u8>; this named type is the opt-in.
type Binary []byte

// Path is a filesystem path exchanged with the host, lowering to the WIT path
// type. It is unconstrained: any direction, file or directory.
type Path string

// QuantityUnit describes the unit constraints of a [Quantity]. Implement it on a
// marker type; the SDK reads it from the type parameter's zero value, so the
// methods must not depend on any state.
type QuantityUnit interface {
	// BaseUnit is the canonical unit, such as "kg", "m", "s" or "B".
	BaseUnit() string
	// AllowedSuffixes are the units accepted in addition to the base unit. An
	// empty list accepts only the base unit; a non-empty list replaces it, so
	// include the base unit to keep it valid.
	AllowedSuffixes() []string
}

// Quantity is a fixed-point measurement carrying its unit, lowering to the WIT
// quantity type. The numeric value is Mantissa x 10^-Scale.
//
//	type Bytes struct{}
//
//	func (Bytes) BaseUnit() string          { return "B" }
//	func (Bytes) AllowedSuffixes() []string { return nil }
//
//	type ByteQuantity = golem.Quantity[Bytes]
//
//	ByteQuantity{Mantissa: 1500, Scale: 0, Unit: "B"} // 1500 B
type Quantity[U QuantityUnit] struct {
	Mantissa int64
	Scale    int32
	// Unit this value is expressed in. An empty Unit means the base unit.
	Unit string
}

// Quantityish exposes the unit marker's constraints without the caller having
// to know U, mirroring the other sealed plumbing interfaces.
type Quantityish interface {
	quantityUnit() QuantityUnit
	quantityValue() (mantissa int64, scale int32, unit string)
}

func (q Quantity[U]) quantityUnit() QuantityUnit {
	var u U
	return u
}

func (q Quantity[U]) quantityValue() (int64, int32, string) {
	unit := q.Unit
	if unit == "" {
		unit = q.quantityUnit().BaseUnit()
	}
	return q.Mantissa, q.Scale, unit
}

type QuantitySetter interface {
	quantitySetValue(mantissa int64, scale int32, unit string)
}

func (q *Quantity[U]) quantitySetValue(mantissa int64, scale int32, unit string) {
	q.Mantissa, q.Scale, q.Unit = mantissa, scale, unit
}

// ---------------------------------------------------------------------------
// Codec plumbing
// ---------------------------------------------------------------------------

// A reflective codec in another module has to read and build these types
// without knowing their type parameters. The interfaces above are sealed — only
// the types here implement them — and their methods are unexported, so a
// foreign package can assert against an interface but cannot call through it.
//
// These functions close that gap. They are the whole plumbing surface, and they
// are package-level on purpose: keeping them off Option and Result themselves
// means a reader of golem.Option sees IsSome, Get and Unwrap rather than four
// reflect-flavoured methods they must never call.
//
// Each reports whether the value was of the expected kind, so a caller
// classifies and unpacks in one step.

// OptionElem reports the element type of an Option.
func OptionElem(v any) (reflect.Type, bool) {
	o, ok := v.(Optionish)
	if !ok {
		return nil, false
	}
	return o.optionElem(), true
}

// OptionGet reports an Option's value and whether it is present. The returned
// value is addressable only for reading.
func OptionGet(v any) (elem reflect.Value, some bool, ok bool) {
	o, isOption := v.(Optionish)
	if !isOption {
		return reflect.Value{}, false, false
	}
	elem, some = o.optionGet()
	return elem, some, true
}

// OptionSetNone empties an Option through a pointer to it.
func OptionSetNone(ptr any) bool {
	o, ok := ptr.(OptionSetter)
	if !ok {
		return false
	}
	o.optionSetNone()
	return true
}

// OptionSetSome marks an Option present and returns the settable element.
func OptionSetSome(ptr any) (reflect.Value, bool) {
	o, ok := ptr.(OptionSetter)
	if !ok {
		return reflect.Value{}, false
	}
	return o.optionSetSome(), true
}

// ResultElems reports a Result's ok and err types.
func ResultElems(v any) (okType, errType reflect.Type, ok bool) {
	r, isResult := v.(Resultish)
	if !isResult {
		return nil, nil, false
	}
	okType, errType = r.resultElems()
	return okType, errType, true
}

// ResultGet reports a Result's payload and whether it is the error arm.
func ResultGet(v any) (elem reflect.Value, isErr bool, ok bool) {
	r, isResult := v.(Resultish)
	if !isResult {
		return reflect.Value{}, false, false
	}
	elem, isErr = r.resultGet()
	return elem, isErr, true
}

// ResultSetOk marks a Result successful and returns the settable ok value.
func ResultSetOk(ptr any) (reflect.Value, bool) {
	r, ok := ptr.(ResultSetter)
	if !ok {
		return reflect.Value{}, false
	}
	return r.resultSetOk(), true
}

// ResultSetErr marks a Result failed and returns the settable err value.
func ResultSetErr(ptr any) (reflect.Value, bool) {
	r, ok := ptr.(ResultSetter)
	if !ok {
		return reflect.Value{}, false
	}
	return r.resultSetErr(), true
}

// QuantityUnitOf reports a Quantity's unit marker, which carries the unit
// constraints without the caller having to know the type parameter.
func QuantityUnitOf(v any) (QuantityUnit, bool) {
	q, ok := v.(Quantityish)
	if !ok {
		return nil, false
	}
	return q.quantityUnit(), true
}

// QuantityParts reports a Quantity's mantissa, scale and unit, resolving an
// empty unit to the marker's base unit.
func QuantityParts(v any) (mantissa int64, scale int32, unit string, ok bool) {
	q, isQuantity := v.(Quantityish)
	if !isQuantity {
		return 0, 0, "", false
	}
	mantissa, scale, unit = q.quantityValue()
	return mantissa, scale, unit, true
}

// QuantitySetParts writes a Quantity through a pointer to it.
func QuantitySetParts(ptr any, mantissa int64, scale int32, unit string) bool {
	q, ok := ptr.(QuantitySetter)
	if !ok {
		return false
	}
	q.quantitySetValue(mantissa, scale, unit)
	return true
}
