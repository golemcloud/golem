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
	pages     [][]apiHost.AgentMetadata
	pageErr   error
	closed    bool
	typeName  string
	forkedOff []uint64
}

func (h *fakeHost) fork() (bool, AgentID, error) { return true, AgentIDFromUUID(UUID{7}), nil }

func (h *fakeHost) getAgents(agentTypeName string, _ witTypes.Option[apiHost.AgentAnyFilter], _ bool) agentPages {
	h.typeName = agentTypeName
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

func (h *fakeHost) forkAgent(source types.AgentId, cutOff uint64) (types.AgentId, error) {
	h.forkedOff = append(h.forkedOff, cutOff)
	return types.AgentId{Uuid: types.Uuid{HighBits: source.Uuid.HighBits, LowBits: source.Uuid.LowBits + 1}}, nil
}

func withFakeHost(t *testing.T, h *fakeHost) {
	t.Helper()
	prev := hostOps
	hostOps = h
	t.Cleanup(func() { hostOps = prev })
}

func TestForkReportsTheSide(t *testing.T) {
	withFakeHost(t, &fakeHost{})
	forked, forkedID := MustForkSelf()
	if !forked || forkedID != AgentIDFromUUID(UUID{7}) {
		t.Fatalf("forked=%v forkedID=%v", forked, forkedID)
	}
}

func TestForkAgentReturnsTheIssuedID(t *testing.T) {
	h := &fakeHost{}
	withFakeHost(t, h)
	forked := MustForkAgent(AgentIDFromUUID(UUID{15: 1}), 9)
	if forked != AgentIDFromUUID(UUID{15: 2}) || !slices.Equal(h.forkedOff, []uint64{9}) {
		t.Fatalf("forked=%v cut-offs=%v", forked, h.forkedOff)
	}
}

func TestAgentFilterFlattensToAnyOfAll(t *testing.T) {
	a := AgentKeyFilter(StringFilterStartsWith, "a")
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
		{apiHost.AgentPropertyFilterKey, apiHost.AgentPropertyFilterStatus, apiHost.AgentPropertyFilterEnv},
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
	md := func(n uint64) apiHost.AgentMetadata {
		return apiHost.AgentMetadata{
			AgentId:       types.AgentId{Uuid: types.Uuid{LowBits: n}},
			AgentTypeName: witTypes.Some("Counter"),
			Fingerprint:   types.Uuid{HighBits: n},
			Env:           []witTypes.Tuple2[string, string]{{F0: "K", F1: "V"}},
			Status:        apiHost.AgentStatusRetrying,
		}
	}
	h := &fakeHost{pages: [][]apiHost.AgentMetadata{{md(1), md(2)}, {md(3)}}, pageErr: errors.New("boom")}
	withFakeHost(t, h)
	var ids []AgentID
	var failed error
	for m, err := range GetAgents("Counter", GetAgentsOptions{}) {
		if err != nil {
			failed = err
			break
		}
		n := m.AgentID.UUID()[15]
		if m.Env["K"] != "V" || m.Status != AgentStatusRetrying || m.AgentTypeName != Some("Counter") ||
			m.Fingerprint != uuidFromWit(types.Uuid{HighBits: uint64(n)}) {
			t.Fatalf("metadata = %+v", m)
		}
		ids = append(ids, m.AgentID)
	}
	want := []AgentID{AgentIDFromUUID(UUID{15: 1}), AgentIDFromUUID(UUID{15: 2}), AgentIDFromUUID(UUID{15: 3})}
	if !slices.Equal(ids, want) || failed == nil || !h.closed || h.typeName != "Counter" {
		t.Fatalf("ids=%v failed=%v closed=%v type=%q", ids, failed, h.closed, h.typeName)
	}

	h = &fakeHost{pageErr: errors.New("boom")}
	withFakeHost(t, h)
	defer func() {
		if recover() == nil {
			t.Fatal("MustGetAgents did not panic on a failing page")
		}
	}()
	for range MustGetAgents("Counter", GetAgentsOptions{}) {
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

func TestSelfMetadataResultPreservesHostErrors(t *testing.T) {
	for _, raw := range []apiHost.AgentOperationError{
		apiHost.MakeAgentOperationErrorPermissionDenied(),
		apiHost.MakeAgentOperationErrorBackendError("metadata unavailable"),
	} {
		_, err := selfMetadataResult(witTypes.Err[apiHost.AgentMetadata, apiHost.AgentOperationError](raw))
		var typed *AgentOperationError
		if !errors.As(err, &typed) || err.Error() != agentOperationErrorFromWit(raw).Error() {
			t.Fatalf("host error lost: %v", err)
		}
	}
}
