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

	"github.com/golemcloud/golem/sdks/go/core/values"
)

// The Go vocabulary for schema values.
//
// The types themselves live in the shared core module, so an agent and an
// external client generated from the same schema get the same Go types and a
// program can hold one domain package for both sides. These are aliases, not
// wrappers: golem.Option[T] *is* values.Option[T].
//
// Go has no alias for a function, so the constructors forward instead. They are
// one line each and exist only so golem.Some reads the way golem.Option does.

// Option is an explicit optional value. See [values.Option].
type Option[T any] = values.Option[T]

// Some returns an Option holding v.
func Some[T any](v T) Option[T] { return values.Some(v) }

// None returns an empty Option.
func None[T any]() Option[T] { return values.None[T]() }

// Result is a value that is either a success or a typed failure. See
// [values.Result].
type Result[Ok any, Err any] = values.Result[Ok, Err]

// ResultError wraps a typed Result error so it satisfies Go's error interface.
type ResultError[E any] = values.ResultError[E]

// Ok returns a successful Result. Both type parameters are explicit because
// neither can be inferred from the argument alone:
//
//	golem.Ok[Money, string](m)
func Ok[O any, E any](v O) Result[O, E] { return values.Ok[O, E](v) }

// Err returns a failed Result.
func Err[O any, E any](e E) Result[O, E] { return values.Err[O, E](e) }

// Char is a single Unicode code point, lowering to the WIT char type.
type Char = values.Char

// URL is a string constrained to a URL, lowering to the WIT url type.
type URL = values.URL

// Text is human-language prose, lowering to the WIT text type.
type Text = values.Text

// Binary is an opaque byte payload, lowering to the WIT binary type.
type Binary = values.Binary

// Path is a filesystem path exchanged with the host, lowering to the WIT path
// type.
type Path = values.Path

// QuantityUnit describes the unit constraints of a [Quantity]. See
// [values.QuantityUnit].
type QuantityUnit = values.QuantityUnit

// Quantity is a fixed-point measurement carrying its unit, lowering to the WIT
// quantity type. See [values.Quantity].
type Quantity[U QuantityUnit] = values.Quantity[U]

// Secret is a handle to a declared config secret, obtained from the agent's
// config ([Config] / [InitContext.Config]). It lowers to the WIT secret
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
// is observed rather than a stale snapshot. A read failure has no in-band
// recovery, so it panics rather than returning an error; the panic surfaces as
// an agent-error. Must be called inside an invocation (it calls the host).
func (s Secret[T]) Get() T {
	if s.read == nil {
		// Only reachable for a zero-value Secret — one never obtained from config
		// (e.g. `var s Secret[string]`). That is a programming mistake, not an
		// operational state; Go cannot forbid the zero value of an exported struct,
		// so this guard turns the otherwise-cryptic nil-closure panic into a clear
		// message.
		panic(fmt.Errorf("golem: Secret has no source; obtain it from the agent's config"))
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
