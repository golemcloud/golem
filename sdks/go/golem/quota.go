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
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// Quota.
//
// A QuotaToken claims a share of a quota-limited resource (an LLM API's
// tokens, say) for this agent. Acquire one per resource, usually in the
// constructor, then reserve before each use and commit what was actually used:
//
//	tok := golem.NewQuotaToken("llm-tokens", 10_000)
//	r, err := tok.Reserve(500)
//	if err != nil { … } // only when the resource rejects instead of throttling
//	n := callModel()
//	r.Commit(n)
//
// When the resource throttles, Reserve waits (the agent may suspend) instead of
// failing. A token is affine: passing one to another agent — as a method
// argument or result — moves it, and this copy is unusable afterwards.

// ErrQuotaTokenMoved reports a quota token used after it was handed on.
var ErrQuotaTokenMoved = errors.New("golem: the quota token was moved to another agent")

// QuotaToken is a claim on a quota-limited resource. Copies share the claim.
type QuotaToken struct{ st *quotaTokenState }

type quotaTokenState struct {
	h *types.QuotaToken
}

//nolint:unused // called from quota_wasm.go
func (t QuotaToken) handle() *types.QuotaToken {
	if t.st == nil || t.st.h == nil {
		panic(ErrQuotaTokenMoved)
	}
	return t.st.h
}

// FailedReservation reports that the resource rejected a reservation.
type FailedReservation struct {
	// EstimatedWait is how long until the amount may be available, when the
	// resource knows.
	EstimatedWait Option[time.Duration]
}

func (e *FailedReservation) Error() string {
	if wait, ok := e.EstimatedWait.Get(); ok {
		return fmt.Sprintf("golem: quota reservation failed; retry in about %s", wait)
	}
	return "golem: quota reservation failed"
}

// Reservation is an amount reserved from a token, settled with Commit.
type Reservation struct {
	commit func(used uint64)
	done   bool
}

// Commit settles the reservation with the amount actually used, which may be
// less than or more than reserved. Committing twice does nothing.
func (r *Reservation) Commit(used uint64) {
	if r.done {
		return
	}
	r.done = true
	r.commit(used)
}

// WithReservation reserves amount, runs use and commits what it reports as
// used. The error is a [*FailedReservation] when the resource rejects the
// reservation.
func WithReservation[T any](t QuotaToken, amount uint64, use func(*Reservation) (used uint64, value T)) (T, error) {
	r, err := t.Reserve(amount)
	if err != nil {
		var zero T
		return zero, err
	}
	used, value := use(r)
	r.Commit(used)
	return value, nil
}

// quotaish recognises QuotaToken in the deriver.
type quotaish interface{ quotaToken() QuotaToken }

func (t QuotaToken) quotaToken() QuotaToken { return t }

// compileQuotaToken lowers QuotaToken to the WIT quota-token type. Encoding
// moves the token; decoding adopts the received one.
func compileQuotaToken(c *codec) {
	c.body = func(*graphBuilder) types.SchemaTypeBody {
		return types.MakeSchemaTypeBodyQuotaTokenType(types.QuotaTokenSpec{ResourceName: witTypes.None[string]()})
	}
	c.encode = func(b *valBuilder, v reflect.Value) int32 {
		t := v.Interface().(QuotaToken)
		if t.st == nil || t.st.h == nil {
			panic(&encodeError{ErrQuotaTokenMoved.Error()})
		}
		h := t.st.h
		t.st.h = nil
		return b.push(types.MakeSchemaValueNodeQuotaTokenHandle(h))
	}
	c.decode = func(d *decoder, dst reflect.Value, idx int32) error {
		n, err := d.node(idx)
		if err != nil {
			return err
		}
		if n.Tag() != types.SchemaValueNodeQuotaTokenHandle {
			return fmt.Errorf("cannot decode value node (tag %d) into %s", n.Tag(), c.typ)
		}
		dst.Set(reflect.ValueOf(QuotaToken{st: &quotaTokenState{h: n.QuotaTokenHandle()}}))
		return nil
	}
}
