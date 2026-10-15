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
	"errors"
	"fmt"
	"math"
	"reflect"
	"strings"
	"testing"
	"time"
	"unsafe"

	apiRetry "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_retry"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

type step struct {
	verdict stepVerdict
	delay   time.Duration
}

func retryAfter(d time.Duration) step { return step{stepRetry, d} }

var (
	giveUp    = step{verdict: stepGiveUp}
	stepFails = step{verdict: stepError}
)

func mustSchedule(t *testing.T, p Policy) Schedule {
	t.Helper()
	s, err := p.Schedule()
	if err != nil {
		t.Fatalf("Schedule: %v", err)
	}
	return s
}

// stepDelays steps a fresh run of the schedule count times, with no time
// passing and no jitter.
func stepDelays(s Schedule, count int, props map[PropName]any) []step {
	state := newPolicyState(s.root)
	projectedProps := projected(props)
	out := make([]step, count)
	for i := range out {
		verdict, delay := state.step(0, projectedProps, func() float64 { return 0 })
		if verdict != stepRetry {
			delay = 0
		}
		out[i] = step{verdict, delay}
	}
	return out
}

func assertSteps(t *testing.T, name string, got, want []step) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("%s: steps %v, want %v", name, got, want)
	}
}

func TestEveryDelayStrategyAndTransformIsInterpreted(t *testing.T) {
	assertSteps(t, "exponential",
		stepDelays(mustSchedule(t, Exponential(3*time.Millisecond, 2)), 3, nil),
		[]step{retryAfter(3 * time.Millisecond), retryAfter(6 * time.Millisecond), retryAfter(12 * time.Millisecond)})
	assertSteps(t, "fibonacci",
		stepDelays(mustSchedule(t, Fibonacci(2*time.Millisecond, 5*time.Millisecond)), 4, nil),
		[]step{retryAfter(2 * time.Millisecond), retryAfter(5 * time.Millisecond), retryAfter(7 * time.Millisecond), retryAfter(12 * time.Millisecond)})
	transformed := Periodic(80*time.Millisecond).
		Clamp(20*time.Millisecond, 50*time.Millisecond).
		AddDelay(7 * time.Millisecond)
	assertSteps(t, "clamp, add delay", stepDelays(mustSchedule(t, transformed), 1, nil),
		[]step{retryAfter(57 * time.Millisecond)})
	assertSteps(t, "never", stepDelays(mustSchedule(t, Never()), 1, nil), []step{giveUp})
}

func TestSequenceUnionAndIntersectionKeepIndependentAsymmetricState(t *testing.T) {
	left := Periodic(10 * time.Millisecond).MaxRetries(1)
	right := Exponential(20*time.Millisecond, 2)
	assertSteps(t, "and then", stepDelays(mustSchedule(t, left.AndThen(right)), 3, nil),
		[]step{retryAfter(10 * time.Millisecond), retryAfter(20 * time.Millisecond), retryAfter(40 * time.Millisecond)})
	assertSteps(t, "union", stepDelays(mustSchedule(t, left.Union(right)), 3, nil),
		[]step{retryAfter(10 * time.Millisecond), retryAfter(40 * time.Millisecond), retryAfter(80 * time.Millisecond)})
	assertSteps(t, "intersect", stepDelays(mustSchedule(t, left.Intersect(right)), 2, nil),
		[]step{retryAfter(20 * time.Millisecond), giveUp})
}

func TestWithinUsesTheElapsedBoundaryWithoutRejectingACrossingDelay(t *testing.T) {
	state := newPolicyState(mustSchedule(t, Periodic(10*time.Second).Within(5*time.Second)).root)
	noJitter := func() float64 { return 0 }
	if v, d := state.step(4_999_999_999, nil, noJitter); v != stepRetry || d != 10*time.Second {
		t.Fatalf("before the limit: %v %v", v, d)
	}
	if v, _ := state.step(5*time.Second, nil, noJitter); v != stepGiveUp {
		t.Fatalf("at the limit: %v", v)
	}
}

func TestDeterministicJitterIsPositiveAndUsesTheBaseDelay(t *testing.T) {
	state := newPolicyState(mustSchedule(t, Periodic(100*time.Millisecond).WithJitter(0.4).MaxRetries(1)).root)
	if v, d := state.step(0, nil, func() float64 { return 0.25 }); v != stepRetry || d != 110*time.Millisecond {
		t.Fatalf("jittered step: %v %v", v, d)
	}
}

func TestAllPredicateNodesAndCoercionsAreSupported(t *testing.T) {
	props := map[PropName]any{"number": "42", "name": "api-503", "enabled": true}
	number, name := Prop("number"), Prop("name")
	for _, pred := range []Predicate{
		number.Eq(42),
		number.Neq(41),
		number.Gt(40),
		number.Gte(42),
		number.Lt(50),
		number.Lte(42),
		Prop("enabled").Exists(),
		number.OneOf(1, 42),
		name.MatchesGlob("{web,api}-[0-9][0-9][0-9]"),
		name.StartsWith("api-"),
		name.Contains("503"),
		MatchAlways().And(MatchNever().Not()),
		MatchNever().Or(MatchAlways()),
	} {
		got := stepDelays(mustSchedule(t, Immediate().OnlyWhen(pred)), 1, props)
		assertSteps(t, fmt.Sprintf("%+v", *pred.node), got, []step{retryAfter(0)})
	}
}

func TestGlobMatchingFollowsTheCrateTheHostUses(t *testing.T) {
	for _, c := range globCrateCases {
		if got := globMatch(c.glob, c.path); got != c.want {
			t.Errorf("globMatch(%q, %q) = %v, the crate says %v", c.glob, c.path, got, c.want)
		}
	}
	for _, c := range []struct {
		glob, path string
		want       bool
	}{
		{"**/foo", "a/b/foo", true},
		{"**/foo", "xfoo", false},
		{"**/", "a", false},
		{"![", "x", false},
		{`api-\*`, "api-*", true},
		{"フ*/**/*", "フォルダ/aaa.js", true},
		{"**/x", strings.Repeat("a", 100_000), false},
	} {
		if got := globMatch(c.glob, c.path); got != c.want {
			t.Errorf("globMatch(%q, %q) = %v, want %v", c.glob, c.path, got, c.want)
		}
	}
}

func TestPredicateErrorsPropagateWithoutActivatingComposedFallbacks(t *testing.T) {
	filtered := Immediate().OnlyWhen(Prop("status").Eq(503))
	for name, policy := range map[string]Policy{
		"and then":     filtered.AndThen(Immediate()),
		"union, left":  filtered.Union(Immediate()),
		"union, right": Immediate().Union(filtered),
	} {
		assertSteps(t, name, stepDelays(mustSchedule(t, policy), 1, nil), []step{stepFails})
	}
}

// fixedRuntime has no time passing and no jitter, and counts the sleeps.
func fixedRuntime(sleeps *int) scheduleRuntime {
	return scheduleRuntime{
		now:    func() time.Duration { return 0 },
		random: func() float64 { return 0 },
		sleep:  func(time.Duration) { *sleeps++ },
	}
}

type statusError struct{ status int }

func (e statusError) Error() string { return fmt.Sprintf("status %d", e.status) }

func TestRetryProjectsEachFailureAndEventuallySucceeds(t *testing.T) {
	var projectedStatuses []int
	s := mustSchedule(t, Immediate().OnlyWhen(StatusCode.Gte(500)).MaxRetries(4)).
		WithProperties(func(err error) map[PropName]any {
			var se statusError
			if !errors.As(err, &se) {
				t.Fatalf("projected %v", err)
			}
			projectedStatuses = append(projectedStatuses, se.status)
			return map[PropName]any{StatusCode: se.status}
		})
	attempts, sleeps := 0, 0
	err := s.run(func() error {
		attempts++
		if attempts == 3 {
			return nil
		}
		return statusError{500 + attempts}
	}, fixedRuntime(&sleeps))
	if err != nil || attempts != 3 || sleeps != 2 || !reflect.DeepEqual(projectedStatuses, []int{501, 502}) {
		t.Fatalf("err %v, attempts %d, sleeps %d, projected %v", err, attempts, sleeps, projectedStatuses)
	}
}

func TestRetryKeepsTheCountAndTheFinalError(t *testing.T) {
	attempts, sleeps := 0, 0
	var last error
	err := mustSchedule(t, Immediate().MaxRetries(2)).run(func() error {
		attempts++
		last = statusError{attempts}
		return last
	}, fixedRuntime(&sleeps))
	if attempts != 3 || err != last {
		t.Fatalf("attempts %d, err %v, the last error was %v", attempts, err, last)
	}
}

func TestRetryStopsWhenAFilteredPredicateStopsMatching(t *testing.T) {
	s := mustSchedule(t, Immediate().OnlyWhen(StatusCode.Eq(503)).MaxRetries(5)).
		WithProperties(func(err error) map[PropName]any {
			return map[PropName]any{StatusCode: err.(statusError).status}
		})
	attempts, sleeps := 0, 0
	err := s.run(func() error {
		attempts++
		if attempts == 1 {
			return statusError{503}
		}
		return statusError{404}
	}, fixedRuntime(&sleeps))
	if attempts != 2 || err != (statusError{404}) {
		t.Fatalf("attempts %d, err %v", attempts, err)
	}
}

func TestElapsedTimeIsMeasuredFromBeforeTheFirstAttempt(t *testing.T) {
	times := []time.Duration{0, 9, 10}
	rt := scheduleRuntime{
		now: func() time.Duration {
			now := times[0]
			times = times[1:]
			return now
		},
		random: func() float64 { return 0 },
		sleep:  func(time.Duration) {},
	}
	attempts := 0
	err := mustSchedule(t, Immediate().Within(10)).run(func() error {
		attempts++
		return statusError{attempts}
	}, rt)
	if attempts != 2 || err != (statusError{2}) {
		t.Fatalf("attempts %d, err %v", attempts, err)
	}
}

func TestAZeroRetryBudgetStillRunsTheFirstAttempt(t *testing.T) {
	attempts, sleeps := 0, 0
	err := mustSchedule(t, Immediate().MaxRetries(0)).run(func() error {
		attempts++
		return errors.New("first failure")
	}, fixedRuntime(&sleeps))
	if attempts != 1 || err == nil || err.Error() != "first failure" || sleeps != 0 {
		t.Fatalf("attempts %d, err %v, sleeps %d", attempts, err, sleeps)
	}
}

func TestRetryValueReturnsTheResultOfTheAttemptThatSucceeded(t *testing.T) {
	attempts := 0
	s := mustSchedule(t, Immediate().MaxRetries(3))
	// RetryValue goes through the host runtime; an immediate policy never
	// sleeps, so it runs natively too.
	got, err := s.RetryValue(func() (string, error) {
		attempts++
		if attempts < 3 {
			return "partial", errors.New("not yet")
		}
		return "done", nil
	})
	if got != "done" || err != nil || attempts != 3 {
		t.Fatalf("got %q, %v after %d attempts", got, err, attempts)
	}
	got, err = mustSchedule(t, Never()).RetryValue(func() (string, error) { return "ignored", errors.New("no") })
	if got != "" || err == nil {
		t.Fatalf("a failed run returned %q, %v", got, err)
	}
}

// A policy from the host is checked like one built here before it runs.
func TestAHostPolicyIsCheckedBeforeItRuns(t *testing.T) {
	decode := func(nodes ...apiRetry.PolicyNode) Policy {
		t.Helper()
		p, err := decodePolicy(apiRetry.RetryPolicy{Nodes: nodes})
		if err != nil {
			t.Fatalf("decode: %v", err)
		}
		return p
	}
	if _, err := decode(apiRetry.MakePolicyNodeClampDelay(apiRetry.ClampConfig{MinDelay: 2, MaxDelay: 1, Inner: 1}),
		apiRetry.MakePolicyNodeImmediate()).Schedule(); err == nil || !strings.Contains(err.Error(), "Clamp") {
		t.Fatalf("an inverted clamp: %v", err)
	}
	for _, factor := range []float64{0, -1, math.NaN(), math.Inf(1), math.Inf(-1)} {
		_, err := decode(apiRetry.MakePolicyNodeExponential(apiRetry.ExponentialConfig{BaseDelay: 1, Factor: factor})).Schedule()
		if err == nil || !strings.Contains(err.Error(), "Exponential factor") {
			t.Fatalf("exponential factor %v: %v", factor, err)
		}
	}
	for _, factor := range []float64{-1, math.NaN(), math.Inf(1), math.Inf(-1)} {
		_, err := decode(apiRetry.MakePolicyNodeJitter(apiRetry.JitterConfig{Factor: factor, Inner: 1}),
			apiRetry.MakePolicyNodeImmediate()).Schedule()
		if err == nil || !strings.Contains(err.Error(), "WithJitter factor") {
			t.Fatalf("jitter factor %v: %v", factor, err)
		}
	}
	zeroJitter := decode(apiRetry.MakePolicyNodeJitter(apiRetry.JitterConfig{Factor: 0, Inner: 1}),
		apiRetry.MakePolicyNodePeriodic(1))
	assertSteps(t, "zero jitter", stepDelays(mustSchedule(t, zeroJitter), 1, nil), []step{retryAfter(1)})
	if _, err := (Policy{}).Schedule(); err == nil {
		t.Fatal("a zero Policy got a schedule")
	}
}

func TestPolicyDepthAndExpansionAreBoundedButSharedStateIsIndependent(t *testing.T) {
	deep := Never()
	for range maxScheduleDepth + 1 {
		deep = deep.AddDelay(0)
	}
	if _, err := deep.Schedule(); !errors.Is(err, ErrPolicyTooComplex) {
		t.Fatalf("a deep policy: %v", err)
	}

	expansive := Immediate()
	for range 13 {
		expansive = expansive.Union(expansive)
	}
	if _, err := expansive.Schedule(); !errors.Is(err, ErrPolicyTooComplex) {
		t.Fatalf("an expansive policy: %v", err)
	}

	members := make([]any, maxSchedulePayloadBytes/int(unsafe.Sizeof(apiRetry.PredicateValue{}))+1)
	for i := range members {
		members[i] = 0
	}
	if _, err := Immediate().OnlyWhen(Prop("value").OneOf(members...)).Schedule(); !errors.Is(err, ErrPolicyTooComplex) {
		t.Fatalf("an oversized set: %v", err)
	}

	shared := Exponential(20*time.Millisecond, 2)
	assertSteps(t, "shared", stepDelays(mustSchedule(t, shared.Union(shared)), 2, nil),
		[]step{retryAfter(20 * time.Millisecond), retryAfter(40 * time.Millisecond)})

	sharedHost, err := decodePolicy(apiRetry.RetryPolicy{Nodes: []apiRetry.PolicyNode{
		apiRetry.MakePolicyNodePolicyUnion(witTypes.Tuple2[int32, int32]{F0: 1, F1: 1}),
		apiRetry.MakePolicyNodeExponential(apiRetry.ExponentialConfig{BaseDelay: 20_000_000, Factor: 2}),
	}})
	if err != nil {
		t.Fatal(err)
	}
	assertSteps(t, "shared from the host", stepDelays(mustSchedule(t, sharedHost), 2, nil),
		[]step{retryAfter(20 * time.Millisecond), retryAfter(40 * time.Millisecond)})
}

// Each run starts the policy from the beginning.
func TestEachRunStartsThePolicyAgain(t *testing.T) {
	s := mustSchedule(t, Immediate().MaxRetries(1))
	for range 2 {
		attempts, sleeps := 0, 0
		_ = s.run(func() error { attempts++; return errors.New("no") }, fixedRuntime(&sleeps))
		if attempts != 2 {
			t.Fatalf("a run made %d attempts, want 2", attempts)
		}
	}
}

func TestAScheduleKeepsTheOneOfValuesItWasCheckedWith(t *testing.T) {
	values := []any{503}
	s := mustSchedule(t, Immediate().OnlyWhen(StatusCode.OneOf(values...)).MaxRetries(1)).
		WithProperties(func(err error) map[PropName]any {
			return map[PropName]any{StatusCode: err.(statusError).status}
		})
	values[0] = time.Now()
	attempts, sleeps := 0, 0
	_ = s.run(func() error {
		attempts++
		return statusError{503}
	}, fixedRuntime(&sleeps))
	if attempts != 2 {
		t.Fatalf("attempts %d", attempts)
	}
}

func TestAnUnsupportedPropertyValuePanics(t *testing.T) {
	defer func() {
		if r := recover(); r == nil || !strings.Contains(fmt.Sprint(r), "unsupported predicate value type") {
			t.Fatalf("recovered %v", r)
		}
	}()
	sleeps := 0
	s := mustSchedule(t, Immediate().MaxRetries(1)).
		WithProperties(func(error) map[PropName]any { return map[PropName]any{"when": time.Now()} })
	_ = s.run(func() error { return errors.New("no") }, fixedRuntime(&sleeps))
}
