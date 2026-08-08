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

package retry

import (
	"math"
	"reflect"
	"strings"
	"testing"
	"time"

	apiRetry "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_retry"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// TestPolicyLoweringStructure — a nested policy lowers to the expected flattened
// node array with root at index 0 and correct child indices (mirrors the Rust
// reference test): CountBox(0) -> FilteredOn(1) -> ClampDelay(2) -> Exponential(3).
func TestPolicyLoweringStructure(t *testing.T) {
	pol := Exponential(200*time.Millisecond, 2.0).
		Clamp(100*time.Millisecond, 5*time.Second).
		OnlyWhen(StatusCode.OneOf(502, 503, 504)).
		MaxRetries(3)

	raw, err := pol.lower()
	if err != nil {
		t.Fatalf("lower: %v", err)
	}
	if len(raw.Nodes) != 4 {
		t.Fatalf("nodes len = %d, want 4", len(raw.Nodes))
	}
	if raw.Nodes[0].Tag() != apiRetry.PolicyNodeCountBox {
		t.Fatalf("nodes[0] tag = %d, want CountBox", raw.Nodes[0].Tag())
	}
	if cb := raw.Nodes[0].CountBox(); cb.MaxRetries != 3 || cb.Inner != 1 {
		t.Fatalf("nodes[0] = %+v, want {MaxRetries:3 Inner:1}", cb)
	}
	if raw.Nodes[1].Tag() != apiRetry.PolicyNodeFilteredOn || raw.Nodes[1].FilteredOn().Inner != 2 {
		t.Fatalf("nodes[1] = %+v, want FilteredOn{Inner:2}", raw.Nodes[1])
	}
	if raw.Nodes[2].Tag() != apiRetry.PolicyNodeClampDelay {
		t.Fatalf("nodes[2] tag = %d, want ClampDelay", raw.Nodes[2].Tag())
	}
	if cl := raw.Nodes[2].ClampDelay(); cl.Inner != 3 || cl.MinDelay != uint64(100*time.Millisecond) || cl.MaxDelay != uint64(5*time.Second) {
		t.Fatalf("nodes[2] = %+v", cl)
	}
	if raw.Nodes[3].Tag() != apiRetry.PolicyNodeExponential {
		t.Fatalf("nodes[3] tag = %d, want Exponential", raw.Nodes[3].Tag())
	}
	if e := raw.Nodes[3].Exponential(); e.BaseDelay != uint64(200*time.Millisecond) || e.Factor != 2.0 {
		t.Fatalf("nodes[3] = %+v", e)
	}

	// The embedded predicate (from OnlyWhen) is a self-contained PropIn.
	pred := raw.Nodes[1].FilteredOn().Predicate
	if len(pred.Nodes) != 1 || pred.Nodes[0].Tag() != apiRetry.PredicateNodePropIn {
		t.Fatalf("embedded predicate = %+v, want single PropIn", pred.Nodes)
	}
	if in := pred.Nodes[0].PropIn(); in.PropertyName != "status-code" || len(in.Values) != 3 {
		t.Fatalf("PropIn = %+v", in)
	}
}

// TestPredicateLoweringStructure — And lowers to a combinator at root referencing
// its two operands by index.
func TestPredicateLoweringStructure(t *testing.T) {
	pred := Verb.Eq("GET").And(StatusCode.Gte(500))
	raw, err := pred.lower()
	if err != nil {
		t.Fatalf("lower: %v", err)
	}
	if len(raw.Nodes) != 3 {
		t.Fatalf("nodes len = %d, want 3", len(raw.Nodes))
	}
	if raw.Nodes[0].Tag() != apiRetry.PredicateNodePredAnd {
		t.Fatalf("nodes[0] tag = %d, want PredAnd", raw.Nodes[0].Tag())
	}
	if a := raw.Nodes[0].PredAnd(); a.F0 != 1 || a.F1 != 2 {
		t.Fatalf("PredAnd = %+v, want {1,2}", a)
	}
	if raw.Nodes[1].Tag() != apiRetry.PredicateNodePropEq || raw.Nodes[1].PropEq().PropertyName != "verb" {
		t.Fatalf("nodes[1] = %+v", raw.Nodes[1])
	}
	if raw.Nodes[2].Tag() != apiRetry.PredicateNodePropGte {
		t.Fatalf("nodes[2] tag = %d, want PropGte", raw.Nodes[2].Tag())
	}
	if v := raw.Nodes[1].PropEq().Value; v.Tag() != apiRetry.PredicateValueText || v.Text() != "GET" {
		t.Fatalf("verb value = %+v", v)
	}
	if v := raw.Nodes[2].PropGte().Value; v.Tag() != apiRetry.PredicateValueInteger || v.Integer() != 500 {
		t.Fatalf("status value = %+v", v)
	}
}

// TestNamedDefaults — Named defaults to priority 0 and an always-applies predicate.
func TestNamedDefaults(t *testing.T) {
	raw, err := Named("p", Immediate()).lower()
	if err != nil {
		t.Fatalf("lower: %v", err)
	}
	if raw.Name != "p" || raw.Priority != 0 {
		t.Fatalf("name/priority = %q/%d", raw.Name, raw.Priority)
	}
	if len(raw.Predicate.Nodes) != 1 || raw.Predicate.Nodes[0].Tag() != apiRetry.PredicateNodePredTrue {
		t.Fatalf("default predicate = %+v, want single PredTrue", raw.Predicate.Nodes)
	}
}

func TestValidationErrors(t *testing.T) {
	cases := []struct {
		name string
		np   NamedPolicy
		want string
	}{
		{"zero policy", Named("x", Policy{}), "zero) Policy"},
		{"exponential factor 0", Named("x", Exponential(time.Second, 0)), "factor must be finite and > 0"},
		{"exponential factor inf", Named("x", Exponential(time.Second, math.Inf(1))), "factor must be finite"},
		{"jitter negative", Named("x", Immediate().WithJitter(-1)), "factor must be finite and >= 0"},
		{"clamp min>max", Named("x", Immediate().Clamp(2*time.Second, time.Second)), "must be <= max"},
		{"negative duration", Named("x", Periodic(-time.Second)), "must be non-negative"},
		{"bad predicate value", Named("x", Immediate().OnlyWhen(StatusCode.Eq(1.5))), "unsupported predicate value type"},
		{"integer overflow", Named("x", Immediate().OnlyWhen(StatusCode.Eq(uint64(math.MaxInt64) + 1))), "overflows int64"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			err := c.np.Validate()
			if err == nil {
				t.Fatalf("expected error containing %q, got nil", c.want)
			}
			if !strings.Contains(err.Error(), c.want) {
				t.Fatalf("error %q does not contain %q", err.Error(), c.want)
			}
		})
	}
}

// TestValidPolicyValidates — a well-formed policy passes Validate.
func TestValidPolicyValidates(t *testing.T) {
	np := Named("ok", Exponential(time.Second, 2).WithJitter(0.5).MaxRetries(5)).
		WithPriority(7).
		AppliesWhen(Verb.Eq("GET").Or(Verb.Eq("HEAD")))
	if err := np.Validate(); err != nil {
		t.Fatalf("Validate: %v", err)
	}
}

// TestRoundTrip — lower -> decode -> lower yields an identical node graph, for
// both the policy and its embedded/applicability predicates.
func TestRoundTrip(t *testing.T) {
	np := Named("rt", Exponential(200*time.Millisecond, 2).
		Clamp(0, time.Second).
		AddDelay(50*time.Millisecond).
		OnlyWhen(ErrorType.Contains("timeout")).
		MaxRetries(3).
		AndThen(Periodic(time.Second).Within(time.Minute))).
		WithPriority(9).
		AppliesWhen(StatusCode.Gte(500).And(URIHost.MatchesGlob("*.example.com")).Not())

	raw1, err := np.lower()
	if err != nil {
		t.Fatalf("lower1: %v", err)
	}
	decoded, err := decodeNamed(raw1)
	if err != nil {
		t.Fatalf("decode: %v", err)
	}
	raw2, err := decoded.lower()
	if err != nil {
		t.Fatalf("lower2: %v", err)
	}
	if !reflect.DeepEqual(raw1.Policy.Nodes, raw2.Policy.Nodes) {
		t.Fatalf("policy round-trip mismatch:\n %+v\n %+v", raw1.Policy.Nodes, raw2.Policy.Nodes)
	}
	if !reflect.DeepEqual(raw1.Predicate.Nodes, raw2.Predicate.Nodes) {
		t.Fatalf("predicate round-trip mismatch:\n %+v\n %+v", raw1.Predicate.Nodes, raw2.Predicate.Nodes)
	}
	if raw2.Name != "rt" || raw2.Priority != 9 {
		t.Fatalf("named fields lost: %q/%d", raw2.Name, raw2.Priority)
	}
}

func TestDecodeGuards(t *testing.T) {
	// empty node list
	if _, err := decodePolicy(apiRetry.RetryPolicy{}); err == nil || !strings.Contains(err.Error(), "empty policy node list") {
		t.Fatalf("empty: %v", err)
	}
	// dangling index: single CountBox pointing at index 5
	dangling := apiRetry.RetryPolicy{Nodes: []apiRetry.PolicyNode{
		apiRetry.MakePolicyNodeCountBox(apiRetry.CountBoxConfig{MaxRetries: 1, Inner: 5}),
	}}
	if _, err := decodePolicy(dangling); err == nil || !strings.Contains(err.Error(), "out of range") {
		t.Fatalf("dangling: %v", err)
	}
	// cycle: node0 -> node1 -> node0
	cyclic := apiRetry.RetryPolicy{Nodes: []apiRetry.PolicyNode{
		apiRetry.MakePolicyNodeCountBox(apiRetry.CountBoxConfig{MaxRetries: 1, Inner: 1}),
		apiRetry.MakePolicyNodeCountBox(apiRetry.CountBoxConfig{MaxRetries: 1, Inner: 0}),
	}}
	if _, err := decodePolicy(cyclic); err == nil || !strings.Contains(err.Error(), "cycle") {
		t.Fatalf("cycle: %v", err)
	}
	// predicate combinator with a dangling operand
	badPred := apiRetry.RetryPredicate{Nodes: []apiRetry.PredicateNode{
		apiRetry.MakePredicateNodePredAnd(witTypes.Tuple2[int32, int32]{F0: 1, F1: 9}),
		apiRetry.MakePredicateNodePredTrue(),
	}}
	if _, err := decodePredicate(badPred); err == nil || !strings.Contains(err.Error(), "out of range") {
		t.Fatalf("bad predicate: %v", err)
	}
}
