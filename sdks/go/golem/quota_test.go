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
	"reflect"
	"testing"
	"time"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type BudgetIn struct {
	Name  string
	Token QuotaToken
}

// TestAQuotaTokenMovesWhenSent — a token travels as the quota-token type, and
// sending it moves it: the sender's copy is unusable, the receiver's adopts it.
func TestAQuotaTokenMovesWhenSent(t *testing.T) {
	h := &types.QuotaToken{}
	in := BudgetIn{Name: "llm", Token: QuotaToken{st: &quotaTokenState{h: h}}}
	sender := in.Token

	got := roundTrip(t, in)
	if got.Name != "llm" || got.Token.st == nil || got.Token.st.h != h {
		t.Fatalf("the receiver got %+v", got)
	}
	func() {
		defer func() {
			if r := recover(); !errors.Is(asError(r), ErrQuotaTokenMoved) {
				t.Errorf("using a moved token gave %v", r)
			}
		}()
		sender.handle()
	}()

	c := defs.compile(reflect.TypeFor[QuotaToken]())
	g := graphBuilder{d: defs}
	root := g.node(c)
	if body := g.build().TypeNodes[root].Body; body.Tag() != types.SchemaTypeBodyQuotaTokenType || body.QuotaTokenType().ResourceName.IsSome() {
		t.Errorf("a token is typed as tag %d", body.Tag())
	}
}

func TestFailedReservationSaysWhenToRetry(t *testing.T) {
	err := error(&FailedReservation{EstimatedWait: Some(2 * time.Second)})
	if err.Error() != "golem: quota reservation failed; retry in about 2s" {
		t.Errorf("message %q", err)
	}
	if (&FailedReservation{EstimatedWait: None[time.Duration]()}).Error() != "golem: quota reservation failed" {
		t.Error("message without an estimate")
	}
}

func asError(r any) error {
	err, _ := r.(error)
	return err
}
