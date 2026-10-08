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
	"github.com/golemcloud/golem/sdks/go/golem/internal/engine"
	"reflect"
	"testing"

	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
)

type CardsIn struct {
	Plain PermissionCard
	Open  PolymorphicPermissionCard
}

// TestPermissionCardsMoveWhenSent — both card types travel as permission-card
// with their polymorphic flag, and sending one moves it.
func TestPermissionCardsMoveWhenSent(t *testing.T) {
	plain, open := &types.PermissionCard{}, &types.PermissionCard{}
	in := CardsIn{Plain: PermissionCard{st: &permissionCardState{h: plain}}, Open: PolymorphicPermissionCard{st: &permissionCardState{h: open}}}
	sent := in

	got := roundTrip(t, in)
	if got.Plain.st.h != plain || got.Open.st.h != open {
		t.Fatalf("the receiver got %+v", got)
	}
	if sent.Plain.st.h != nil || sent.Open.st.h != nil {
		t.Error("the sender still holds the cards")
	}

	for typ, want := range map[reflect.Type]bool{
		reflect.TypeFor[PermissionCard]():            false,
		reflect.TypeFor[PolymorphicPermissionCard](): true,
	} {
		g := engine.GraphBuilder{E: defs.Engine}
		root := g.Node(defs.Compile(typ))
		body := g.Build().TypeNodes[root].Body
		if body.Tag() != types.SchemaTypeBodyPermissionCardType || body.PermissionCardType().Polymorphic != want {
			t.Errorf("%s is typed as tag %d", typ, body.Tag())
		}
	}

	func() {
		defer func() {
			if r := recover(); r == nil {
				t.Error("sending a moved card succeeded")
			}
		}()
		roundTrip(t, sent)
	}()
}
