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
	"strings"
	"testing"
	"time"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	host "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
)

// snapCustomState — A state that controls its own serialization — the only way to capture
// unexported fields.
type snapCustomState struct{ count int64 }

func (s *snapCustomState) Save() ([]byte, error) { return []byte{byte(s.count)}, nil }
func (s *snapCustomState) Load(b []byte) error   { s.count = int64(b[0]); return nil }

// restore splits a saved snapshot the way load-snapshot does and loads its
// state into dst, returning the principal it carried.
func restore(t *testing.T, dst any, snap host.Snapshot) Principal {
	t.Helper()
	principal, state, err := splitSnapshot(snap)
	if err != nil {
		t.Fatalf("split: %v", err)
	}
	if err := loadState(dst, state); err != nil {
		t.Fatalf("load: %v", err)
	}
	return principal
}

var snapPrincipal = OidcPrincipal{Sub: "u-1", Issuer: "https://id.example", Email: Some("a@example.com"), Claims: "{}"}

func TestSnapshotViaSnapshotter(t *testing.T) {
	src := &snapCustomState{count: 42}
	snap, err := saveState(src, snapPrincipal)
	if err != nil {
		t.Fatalf("save: %v", err)
	}
	if snap.MimeType != snapshotRawMIME {
		t.Errorf("MIME = %q, want %q", snap.MimeType, snapshotRawMIME)
	}
	dst := &snapCustomState{}
	if p := restore(t, dst, snap); p != snapPrincipal {
		t.Errorf("principal = %#v, want %#v", p, snapPrincipal)
	}
	if dst.count != 42 {
		t.Errorf("round trip lost the count: got %d", dst.count)
	}
}

func TestSnapshotReflectiveDefaultCoversExportedFields(t *testing.T) {
	type State struct {
		Total  int64
		Region string
	}
	src := &State{Total: 7, Region: "eu"}
	snap, err := saveState(src, snapPrincipal)
	if err != nil {
		t.Fatalf("save: %v", err)
	}
	if snap.MimeType != snapshotJSONMIME {
		t.Errorf("MIME = %q, want %q", snap.MimeType, snapshotJSONMIME)
	}
	dst := &State{}
	if p := restore(t, dst, snap); p != snapPrincipal {
		t.Errorf("principal = %#v, want %#v", p, snapPrincipal)
	}
	if *dst != *src {
		t.Errorf("round trip = %+v, want %+v", *dst, *src)
	}
}

// TestSnapshotReflectiveDefaultDropsUnexportedFields — The documented caveat: without a Snapshotter, unexported fields are invisible
// to reflection and are NOT captured.
func TestSnapshotReflectiveDefaultDropsUnexportedFields(t *testing.T) {
	src := &struct{ count int64 }{count: 99}
	snap, err := saveState(src, snapPrincipal)
	if err != nil {
		t.Fatalf("save: %v", err)
	}
	_, state, err := splitSnapshot(snap)
	if err != nil {
		t.Fatalf("split: %v", err)
	}
	if string(state.Payload) != "{}" {
		t.Errorf("expected empty JSON for all-unexported state, got %q", state.Payload)
	}
}

func TestSnapshotPolicyMapsToWit(t *testing.T) {
	cases := []struct {
		name    string
		policy  SnapshotPolicy
		enabled bool
		cfgTag  uint8
		amount  uint64
	}{
		{"disabled", SnapshotDisabled, false, 0, 0},
		{"default", SnapshotDefault, true, common.SnapshottingConfigDefault, 0},
		{"periodic", SnapshotPeriodic(2 * time.Second), true, common.SnapshottingConfigPeriodic, uint64(2 * time.Second)},
		{"everyN", SnapshotEveryN(5), true, common.SnapshottingConfigEveryNInvocation, 5},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			type Id struct{ Name string }
			type St struct{}
			withDefs(t, func(d *definitions) {
				def := defineAgentInto[Id, NoConfig](d, Spec{Name: "A", Snapshot: c.policy})
				impl := implementInto[Id, St, NoConfig](d, def, simpleNewState[Id, St](func(Id) *St { return &St{} }), false)
				impl.Handle(def.Method[Unit, Unit]("m"), func(*Context[St], Unit) Unit { return Unit{} })
				types, errs := d.discover()
				if len(errs) != 0 {
					t.Fatalf("errs: %v", errs)
				}
				s := types[0].Snapshotting
				if !c.enabled {
					if s.Tag() != common.SnapshottingDisabled {
						t.Fatalf("want disabled, got tag %d", s.Tag())
					}
					return
				}
				if s.Tag() != common.SnapshottingEnabled {
					t.Fatalf("want enabled, got tag %d", s.Tag())
				}
				cfg := s.Enabled()
				if cfg.Tag() != c.cfgTag {
					t.Fatalf("config tag = %d, want %d", cfg.Tag(), c.cfgTag)
				}
				switch c.cfgTag {
				case common.SnapshottingConfigPeriodic:
					if cfg.Periodic() != c.amount {
						t.Errorf("periodic = %d, want %d", cfg.Periodic(), c.amount)
					}
				case common.SnapshottingConfigEveryNInvocation:
					if uint64(cfg.EveryNInvocation()) != c.amount {
						t.Errorf("everyN = %d, want %d", cfg.EveryNInvocation(), c.amount)
					}
				}
			})
		})
	}
}

type restoreID struct{ Name string }

type rState struct {
	Count    int64
	greeting string // runtime-only: rebuilt by the restore hook
}

// TestRestoreSkipsTheConstructor — a restore loads into a zero state and runs
// the OnRestore hook; the constructor, whose effects are already in the
// history the snapshot replaces, does not run.
func TestRestoreSkipsTheConstructor(t *testing.T) {
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[restoreID, NoConfig](d, Spec{Name: "R"})
		constructed := 0
		impl := implementInto[restoreID, rState, NoConfig](d, def, simpleNewState[restoreID, rState](func(restoreID) *rState {
			constructed++
			return &rState{Count: 100}
		}), false)
		impl.OnRestore(func(ic *InitContext[restoreID, rState, NoConfig], s *rState) error {
			s.greeting = "hello " + ic.ID().Name + " from " + ic.AgentID()
			return nil
		})
		impl.Handle(def.Method[Unit, int64]("count"), func(ctx *Context[rState], _ Unit) int64 { return ctx.State.Count })
		noDefErrs(t, d)

		snap, err := saveState(&rState{Count: 7, greeting: "lost"}, snapPrincipal)
		if err != nil {
			t.Fatal(err)
		}
		_, state, err := splitSnapshot(snap)
		if err != nil {
			t.Fatal(err)
		}
		e := d.agents["R"]
		idVal := reflect.ValueOf(restoreID{Name: "ada"})
		got, err := restoreState(e, idVal, `R("ada")`, snapPrincipal, state)
		if err != nil {
			t.Fatal(err)
		}
		s := got.(*rState)
		if constructed != 0 {
			t.Errorf("the constructor ran %d times on restore", constructed)
		}
		if s.Count != 7 || s.greeting != `hello ada from R("ada")` {
			t.Errorf("restored state = %+v", *s)
		}
	})
}

// TestRestoreHookErrorFailsTheRestore — an OnRestore error is the restore's.
func TestRestoreHookErrorFailsTheRestore(t *testing.T) {
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[restoreID, NoConfig](d, Spec{Name: "R"})
		impl := implementInto[restoreID, rState, NoConfig](d, def, simpleNewState[restoreID, rState](func(restoreID) *rState {
			return &rState{}
		}), false)
		impl.OnRestore(func(*InitContext[restoreID, rState, NoConfig], *rState) error {
			return errors.New("dependency gone")
		})
		snap, _ := saveState(&rState{}, snapPrincipal)
		_, state, _ := splitSnapshot(snap)
		_, err := restoreState(d.agents["R"], reflect.ValueOf(restoreID{}), "R()", snapPrincipal, state)
		if err == nil || !strings.Contains(err.Error(), "dependency gone") {
			t.Fatalf("err = %v", err)
		}
	})
}

func TestOnRestoreTwiceIsADefinitionError(t *testing.T) {
	withDefs(t, func(d *definitions) {
		def := defineAgentInto[restoreID, NoConfig](d, Spec{Name: "R"})
		impl := implementInto[restoreID, rState, NoConfig](d, def, simpleNewState[restoreID, rState](func(restoreID) *rState {
			return &rState{}
		}), false)
		hook := func(*InitContext[restoreID, rState, NoConfig], *rState) error { return nil }
		impl.OnRestore(hook)
		impl.OnRestore(hook)
		mustDefErr(t, d, "OnRestore already registered")
	})
}
