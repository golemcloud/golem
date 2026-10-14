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
	"slices"
	"testing"
	"time"

	apiHost "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_host"
	types "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_core_types"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// fakeHost stands in for the host in native tests; unset methods panic via the
// embedded nil interface.
type fakeHost struct {
	agentHost
	pages   [][]apiHost.AgentMetadata
	pageErr error
	closed  bool
	strict  []bool
}

func (h *fakeHost) fork() (bool, UUID, error) { return true, UUID{7}, nil }

func (h *fakeHost) getAgents(types.ComponentId, witTypes.Option[apiHost.AgentAnyFilter], bool) agentPages {
	return h
}

func (h *fakeHost) next() ([]apiHost.AgentMetadata, bool, error) {
	if len(h.pages) == 0 {
		return nil, false, h.pageErr
	}
	page := h.pages[0]
	h.pages = h.pages[1:]
	return page, true, nil
}

func (h *fakeHost) close() { h.closed = true }

func (h *fakeHost) resolveAgentID(_, name string, strict bool) (types.AgentId, bool) {
	h.strict = append(h.strict, strict)
	return types.AgentId{AgentId: name}, true
}

func withFakeHost(t *testing.T, h *fakeHost) {
	t.Helper()
	prev := hostOps
	hostOps = h
	t.Cleanup(func() { hostOps = prev })
}

func TestForkReportsTheSide(t *testing.T) {
	withFakeHost(t, &fakeHost{})
	forked, phantom := MustForkSelf()
	if !forked || phantom != (UUID{7}) {
		t.Fatalf("forked=%v phantom=%v", forked, phantom)
	}
}

func TestAgentFilterFlattensToAnyOfAll(t *testing.T) {
	a := AgentNameFilter(StringFilterStartsWith, "a")
	b := AgentStatusFilter(FilterEqual, AgentStatusIdle)
	c := AgentVersionFilter(FilterGreater, 2)
	d := AgentEnvFilter("E", StringFilterEqual, "d")

	// (a ∧ b ∨ c) ∧ d = (a ∧ b ∧ d) ∨ (c ∧ d)
	got := a.And(b).Or(c).And(d).toWit().Some()
	tags := [][]uint8{}
	for _, g := range got.Filters {
		row := []uint8{}
		for _, f := range g.Filters {
			row = append(row, f.Tag())
		}
		tags = append(tags, row)
	}
	want := [][]uint8{
		{apiHost.AgentPropertyFilterName, apiHost.AgentPropertyFilterStatus, apiHost.AgentPropertyFilterEnv},
		{apiHost.AgentPropertyFilterVersion, apiHost.AgentPropertyFilterEnv},
	}
	if !slices.EqualFunc(tags, want, slices.Equal) {
		t.Fatalf("groups = %v, want %v", tags, want)
	}

	if (AgentFilter{}).toWit().IsSome() {
		t.Error("the zero filter must select everything")
	}
	if a.Or(AgentFilter{}).toWit().IsSome() {
		t.Error("or with everything is everything")
	}
	if got := (AgentFilter{}).And(a).toWit().Some(); len(got.Filters) != 1 {
		t.Error("and with everything is the other filter")
	}
	at := time.UnixMilli(1_700_000_000_123)
	if v := AgentCreatedAtFilter(FilterLess, at).anyOf[0][0].CreatedAt().Value; v != 1_700_000_000_123 {
		t.Errorf("created-at = %d, want milliseconds", v)
	}
}

// TestEnumsMatchTheHost — our constants are converted to the host's by value.
func TestEnumsMatchTheHost(t *testing.T) {
	pairs := map[string][2]uint8{
		"FilterEqual":            {uint8(FilterEqual), apiHost.FilterComparatorEqual},
		"FilterLess":             {uint8(FilterLess), apiHost.FilterComparatorLess},
		"StringFilterStartsWith": {uint8(StringFilterStartsWith), apiHost.StringFilterComparatorStartsWith},
		"AgentStatusExited":      {uint8(AgentStatusExited), apiHost.AgentStatusExited},
		"AgentStatusSuspended":   {uint8(AgentStatusSuspended), apiHost.AgentStatusSuspended},
		"UpdateModeSnapshot":     {uint8(UpdateModeSnapshotBased), apiHost.UpdateModeSnapshotBased},
	}
	for name, p := range pairs {
		if p[0] != p[1] {
			t.Errorf("%s = %d, host has %d", name, p[0], p[1])
		}
	}
}

func TestGetAgentsPagesAndStops(t *testing.T) {
	md := func(name string) apiHost.AgentMetadata {
		return apiHost.AgentMetadata{
			AgentId: types.AgentId{AgentId: name},
			Env:     []witTypes.Tuple2[string, string]{{F0: "K", F1: "V"}},
			Status:  apiHost.AgentStatusRetrying,
		}
	}
	h := &fakeHost{pages: [][]apiHost.AgentMetadata{{md("a"), md("b")}, {md("c")}}, pageErr: errors.New("boom")}
	withFakeHost(t, h)
	var names []string
	var failed error
	for m, err := range GetAgents(UUID{}, GetAgentsOptions{}) {
		if err != nil {
			failed = err
			break
		}
		if m.Env["K"] != "V" || m.Status != AgentStatusRetrying {
			t.Fatalf("metadata = %+v", m)
		}
		names = append(names, m.AgentID.AgentID)
	}
	if !slices.Equal(names, []string{"a", "b", "c"}) || failed == nil || !h.closed {
		t.Fatalf("names=%v failed=%v closed=%v", names, failed, h.closed)
	}

	h = &fakeHost{pageErr: errors.New("boom")}
	withFakeHost(t, h)
	defer func() {
		if recover() == nil {
			t.Fatal("MustGetAgents did not panic on a failing page")
		}
	}()
	for range MustGetAgents(UUID{}, GetAgentsOptions{}) {
	}
}

func TestAgentOperationErrorsAreTyped(t *testing.T) {
	var e *AgentOperationError
	if err := agentOperationErrorFromWit(apiHost.MakeAgentOperationErrorPermissionDenied()); !errors.As(err, &e) || e.Kind != AgentOperationPermissionDenied {
		t.Fatalf("permission denied = %v", err)
	}
	if err := agentOperationErrorFromWit(apiHost.MakeAgentOperationErrorBackendError("down")); !errors.As(err, &e) || e.Kind != AgentOperationBackendError || e.Message != "down" {
		t.Fatalf("backend error = %v", err)
	}
}

func TestResolveAgentIDStrictness(t *testing.T) {
	h := &fakeHost{}
	withFakeHost(t, h)
	ResolveAgentID("c", "A()")
	ResolveAgentIDStrict("c", "A()")
	if !slices.Equal(h.strict, []bool{false, true}) {
		t.Fatalf("strict = %v", h.strict)
	}
}
