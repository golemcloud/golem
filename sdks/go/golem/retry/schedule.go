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
	"cmp"
	"crypto/rand"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"strconv"
	"strings"
	"time"
	"unsafe"

	apiRetry "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_retry"
)

// Schedule is a policy the component runs itself, around its own code:
// [Schedule.Retry] calls an operation until it succeeds or the policy gives
// up, and sleeps between the attempts. Unlike [Set] and [With], it installs no
// named policy and the host keeps no retry state for it. The loop is ordinary
// agent code, so its sleeps and the operation's host calls are durable and
// replay like any other.
//
//	sched, err := retry.Exponential(100*time.Millisecond, 2).MaxRetries(4).Schedule()
//	err = sched.Retry(func() error { return charge(order) })
//
// A schedule is a value; each Retry starts the policy from the beginning.
type Schedule struct {
	root    *policyNode
	project func(error) map[PropName]any
}

// The bounds a schedule's policy must stay within, counting a policy or
// predicate shared by several combinators once for each place it appears.
const (
	maxScheduleNodes        = 4_096
	maxSchedulePayloadBytes = 1_048_576
	maxScheduleDepth        = 256
)

// ErrPolicyTooComplex reports a policy too deeply nested or too large to run
// as a [Schedule].
var ErrPolicyTooComplex = errors.New("golem/retry: retry policy is too deeply nested or expansive")

// Schedule checks the policy and returns it as a [Schedule]: the value checks
// of [Set], and bounds on its size. A policy from the host ([ByName],
// [Resolve]) is checked the same way.
func (p Policy) Schedule() (Schedule, error) {
	if p.node == nil {
		return Schedule{}, errors.New("golem/retry: invalid (zero) Policy; build one with a constructor such as retry.Exponential or retry.Periodic")
	}
	var b scheduleBudget
	if err := b.checkPolicy(p.node, 0); err != nil {
		return Schedule{}, err
	}
	return Schedule{root: p.node}, nil
}

// WithProperties returns the schedule with a projection of each failure to the
// properties its [Policy.OnlyWhen] predicates look at. The projection runs
// after every failure, so a predicate follows the current error. A property
// value is a string, an integer type or a bool; any other type panics.
func (s Schedule) WithProperties(project func(err error) map[PropName]any) Schedule {
	s.project = project
	return s
}

// Retry calls op until it returns nil or the schedule gives up, and then
// returns op's last error unchanged. op runs at least once.
func (s Schedule) Retry(op func() error) error {
	return s.run(op, hostRuntime())
}

// RetryValue is [Schedule.Retry] for an operation with a result.
func (s Schedule) RetryValue[T any](op func() (T, error)) (T, error) {
	var out T
	err := s.Retry(func() error {
		v, err := op()
		if err == nil {
			out = v
		}
		return err
	})
	return out, err
}

// scheduleRuntime is what a run needs from its environment: the time since
// the run started, a uniform random value in [0, 1), and a sleep.
type scheduleRuntime struct {
	now    func() time.Duration
	random func() float64
	sleep  func(time.Duration)
}

// hostRuntime reads the clock and randomness through the host and sleeps
// with it, so a replay sees the recorded values and does not sleep again.
func hostRuntime() scheduleRuntime {
	start := time.Now()
	return scheduleRuntime{
		now: func() time.Duration { return time.Since(start) },
		random: func() float64 {
			var b [8]byte
			_, _ = rand.Read(b[:])
			return float64(binary.LittleEndian.Uint64(b[:])>>11) / (1 << 53)
		},
		sleep: time.Sleep,
	}
}

func (s Schedule) run(op func() error, rt scheduleRuntime) error {
	if s.root == nil {
		panic("golem/retry: Retry on a zero Schedule; get one from Policy.Schedule")
	}
	started := rt.now()
	state := newPolicyState(s.root)
	for {
		err := op()
		if err == nil {
			return nil
		}
		var props []namedValue
		if s.project != nil {
			props = projected(s.project(err))
		}
		verdict, delay := state.step(rt.now()-started, props, rt.random)
		if verdict != stepRetry {
			return err
		}
		rt.sleep(delay)
	}
}

// namedValue is one property a failure was projected to.
type namedValue struct {
	name  string
	value apiRetry.PredicateValue
}

func projected(props map[PropName]any) []namedValue {
	out := make([]namedValue, 0, len(props))
	for name, v := range props {
		pv, err := toPredicateValue(v)
		if err != nil {
			panic(fmt.Errorf("golem/retry: property %q: %w", name, err))
		}
		out = append(out, namedValue{string(name), pv})
	}
	return out
}

// ── Checking ─────────────────────────────────────────────────────────────────

type scheduleBudget struct {
	nodes, payload int
}

func (b *scheduleBudget) enter(depth, payload int) error {
	b.nodes++
	b.payload += payload
	if depth > maxScheduleDepth || b.nodes > maxScheduleNodes || b.payload > maxSchedulePayloadBytes {
		return ErrPolicyTooComplex
	}
	return nil
}

func (b *scheduleBudget) checkPolicy(n *policyNode, depth int) error {
	if n == nil {
		return errors.New("golem/retry: incomplete Policy: a combinator was applied to a zero Policy")
	}
	if err := b.enter(depth, 0); err != nil {
		return err
	}
	next := depth + 1
	switch n.kind {
	case pkPeriodic:
		return checkDur("Periodic delay", n.d1)
	case pkExponential:
		if err := checkDur("Exponential base", n.d1); err != nil {
			return err
		}
		if !finitePositive(n.factor) {
			return fmt.Errorf("golem/retry: Exponential factor must be finite and > 0, got %v", n.factor)
		}
		return nil
	case pkFibonacci:
		if err := checkDur("Fibonacci first", n.d1); err != nil {
			return err
		}
		return checkDur("Fibonacci second", n.d2)
	case pkImmediate, pkNever:
		return nil
	case pkCountBox:
		return b.checkPolicy(n.inner, next)
	case pkTimeBox:
		if err := checkDur("Within limit", n.d1); err != nil {
			return err
		}
		return b.checkPolicy(n.inner, next)
	case pkClampDelay:
		if err := checkDur("Clamp min", n.d1); err != nil {
			return err
		}
		if err := checkDur("Clamp max", n.d2); err != nil {
			return err
		}
		if n.d1 > n.d2 {
			return fmt.Errorf("golem/retry: Clamp min %v must not exceed max %v", n.d1, n.d2)
		}
		return b.checkPolicy(n.inner, next)
	case pkAddDelay:
		if err := checkDur("AddDelay", n.d1); err != nil {
			return err
		}
		return b.checkPolicy(n.inner, next)
	case pkJitter:
		if !finiteNonNegative(n.factor) {
			return fmt.Errorf("golem/retry: WithJitter factor must be finite and >= 0, got %v", n.factor)
		}
		return b.checkPolicy(n.inner, next)
	case pkFilteredOn:
		if err := b.checkPredicate(n.pred, next); err != nil {
			return err
		}
		return b.checkPolicy(n.inner, next)
	case pkAndThen, pkUnion, pkIntersect:
		if err := b.checkPolicy(n.left, next); err != nil {
			return err
		}
		return b.checkPolicy(n.right, next)
	default:
		return fmt.Errorf("golem/retry: internal: unknown policy kind %d", n.kind)
	}
}

func (b *scheduleBudget) checkPredicate(n *predicateNode, depth int) error {
	if n == nil {
		return errors.New("golem/retry: incomplete Predicate: a combinator was applied to a zero Predicate")
	}
	payload := len(n.prop) + len(n.pat)
	if n.kind == prIn {
		payload += len(n.vals) * int(unsafe.Sizeof(apiRetry.PredicateValue{}))
	}
	for _, v := range append([]any{n.val}, n.vals...) {
		if s, ok := v.(string); ok {
			payload += len(s)
		}
	}
	if err := b.enter(depth, payload); err != nil {
		return err
	}
	next := depth + 1
	switch n.kind {
	case prEq, prNeq, prGt, prGte, prLt, prLte:
		_, err := toPredicateValue(n.val)
		return err
	case prIn:
		for _, v := range n.vals {
			if _, err := toPredicateValue(v); err != nil {
				return err
			}
		}
		return nil
	case prExists, prMatches, prStartsWith, prContains, prTrue, prFalse:
		return nil
	case prAnd, prOr:
		if err := b.checkPredicate(n.left, next); err != nil {
			return err
		}
		return b.checkPredicate(n.right, next)
	case prNot:
		return b.checkPredicate(n.inner, next)
	default:
		return fmt.Errorf("golem/retry: internal: unknown predicate kind %d", n.kind)
	}
}

// ── Stepping ─────────────────────────────────────────────────────────────────

type stepVerdict uint8

const (
	stepRetry stepVerdict = iota
	stepGiveUp
	// stepError is a predicate that could not be evaluated: a property it
	// needs is missing or cannot be compared. The run gives up, and no
	// fallback of the policy takes over.
	stepError
)

// policyState is a policy with the state of one run: the attempts of a
// strategy, the retries a MaxRetries spent, which side of an AndThen runs.
type policyState struct {
	node              *policyNode
	attempt, attempts uint32
	previous, current time.Duration
	onRight           bool
	inner             *policyState
	left, right       *policyState
}

func newPolicyState(n *policyNode) *policyState {
	if n == nil {
		return nil
	}
	return &policyState{
		node:     n,
		previous: n.d1,
		current:  n.d2,
		inner:    newPolicyState(n.inner),
		left:     newPolicyState(n.left),
		right:    newPolicyState(n.right),
	}
}

func (s *policyState) step(elapsed time.Duration, props []namedValue, random func() float64) (stepVerdict, time.Duration) {
	n := s.node
	switch n.kind {
	case pkPeriodic:
		return stepRetry, n.d1
	case pkExponential:
		delay := scaleDuration(n.d1, math.Pow(n.factor, float64(s.attempt)))
		s.attempt = saturatingInc(s.attempt)
		return stepRetry, delay
	case pkFibonacci:
		var delay time.Duration
		switch s.attempt {
		case 0:
			delay = s.previous
		case 1:
			delay = s.current
		default:
			next := saturatingAdd(s.previous, s.current)
			s.previous, s.current = s.current, next
			delay = next
		}
		s.attempt = saturatingInc(s.attempt)
		return stepRetry, delay
	case pkImmediate:
		return stepRetry, 0
	case pkNever:
		return stepGiveUp, 0
	case pkCountBox:
		if s.attempts >= n.maxTries {
			return stepGiveUp, 0
		}
		s.attempts = saturatingInc(s.attempts)
		return s.inner.step(elapsed, props, random)
	case pkTimeBox:
		if elapsed >= n.d1 {
			return stepGiveUp, 0
		}
		return s.inner.step(elapsed, props, random)
	case pkClampDelay:
		verdict, delay := s.inner.step(elapsed, props, random)
		if verdict != stepRetry {
			return verdict, 0
		}
		return stepRetry, min(max(delay, n.d1), n.d2)
	case pkAddDelay:
		verdict, delay := s.inner.step(elapsed, props, random)
		if verdict != stepRetry {
			return verdict, 0
		}
		return stepRetry, saturatingAdd(delay, n.d1)
	case pkJitter:
		verdict, delay := s.inner.step(elapsed, props, random)
		if verdict != stepRetry || n.factor <= 0 {
			return verdict, delay
		}
		return stepRetry, saturatingAdd(delay, scaleDuration(delay, random()*n.factor))
	case pkFilteredOn:
		ok, err := matches(n.pred, props)
		switch {
		case err != nil:
			return stepError, 0
		case !ok:
			return stepGiveUp, 0
		}
		return s.inner.step(elapsed, props, random)
	case pkAndThen:
		if s.onRight {
			return s.right.step(elapsed, props, random)
		}
		verdict, delay := s.left.step(elapsed, props, random)
		if verdict != stepGiveUp {
			return verdict, delay
		}
		s.onRight = true
		return s.right.step(elapsed, props, random)
	case pkUnion:
		lv, ld := s.left.step(elapsed, props, random)
		rv, rd := s.right.step(elapsed, props, random)
		switch {
		case lv == stepError || rv == stepError:
			return stepError, 0
		case lv == stepRetry && rv == stepRetry:
			return stepRetry, min(ld, rd)
		case lv == stepRetry:
			return stepRetry, ld
		case rv == stepRetry:
			return stepRetry, rd
		}
		return stepGiveUp, 0
	case pkIntersect:
		lv, ld := s.left.step(elapsed, props, random)
		rv, rd := s.right.step(elapsed, props, random)
		switch {
		case lv == stepError || rv == stepError:
			return stepError, 0
		case lv == stepRetry && rv == stepRetry:
			return stepRetry, max(ld, rd)
		}
		return stepGiveUp, 0
	default:
		panic(fmt.Sprintf("golem/retry: internal: unknown policy kind %d", n.kind))
	}
}

// ── Predicates ───────────────────────────────────────────────────────────────

// errUnevaluable is a predicate that needs a missing property, or compares
// values that do not compare.
var errUnevaluable = errors.New("golem/retry: the predicate cannot be evaluated against the properties")

func matches(n *predicateNode, props []namedValue) (bool, error) {
	switch n.kind {
	case prEq, prNeq, prGt, prGte, prLt, prLte:
		actual, err := required(props, n.prop)
		if err != nil {
			return false, err
		}
		expected, err := toPredicateValue(n.val)
		if err != nil {
			return false, err
		}
		order, err := comparePredicateValues(actual, expected)
		if err != nil {
			return false, err
		}
		switch n.kind {
		case prEq:
			return order == 0, nil
		case prNeq:
			return order != 0, nil
		case prGt:
			return order > 0, nil
		case prGte:
			return order >= 0, nil
		case prLt:
			return order < 0, nil
		default:
			return order <= 0, nil
		}
	case prExists:
		_, ok := property(props, n.prop)
		return ok, nil
	case prIn:
		actual, err := required(props, n.prop)
		if err != nil {
			return false, err
		}
		failed := false
		for _, v := range n.vals {
			expected, err := toPredicateValue(v)
			if err != nil {
				return false, err
			}
			order, err := comparePredicateValues(actual, expected)
			switch {
			case err != nil:
				failed = true
			case order == 0:
				return true, nil
			}
		}
		if failed {
			return false, errUnevaluable
		}
		return false, nil
	case prMatches, prStartsWith, prContains:
		actual, err := required(props, n.prop)
		if err != nil {
			return false, err
		}
		text, err := asText(actual)
		if err != nil {
			return false, err
		}
		switch n.kind {
		case prMatches:
			return globMatch(n.pat, text), nil
		case prStartsWith:
			return strings.HasPrefix(text, n.pat), nil
		default:
			return strings.Contains(text, n.pat), nil
		}
	case prAnd:
		left, err := matches(n.left, props)
		if err != nil || !left {
			return false, err
		}
		return matches(n.right, props)
	case prOr:
		left, err := matches(n.left, props)
		if err != nil || left {
			return left, err
		}
		return matches(n.right, props)
	case prNot:
		inner, err := matches(n.inner, props)
		return !inner, err
	case prTrue:
		return true, nil
	case prFalse:
		return false, nil
	default:
		panic(fmt.Sprintf("golem/retry: internal: unknown predicate kind %d", n.kind))
	}
}

// property finds a projected property; the last one with the name wins.
func property(props []namedValue, name string) (apiRetry.PredicateValue, bool) {
	for i := len(props) - 1; i >= 0; i-- {
		if props[i].name == name {
			return props[i].value, true
		}
	}
	return apiRetry.PredicateValue{}, false
}

func required(props []namedValue, name string) (apiRetry.PredicateValue, error) {
	v, ok := property(props, name)
	if !ok {
		return v, errUnevaluable
	}
	return v, nil
}

// comparePredicateValues orders two values as the host does: values of one
// kind directly, a text against an integer as the integer it parses to, and an
// integer against a text as its decimal spelling.
func comparePredicateValues(actual, expected apiRetry.PredicateValue) (int, error) {
	switch {
	case actual.Tag() == apiRetry.PredicateValueInteger && expected.Tag() == apiRetry.PredicateValueInteger:
		return cmp.Compare(actual.Integer(), expected.Integer()), nil
	case actual.Tag() == apiRetry.PredicateValueText && expected.Tag() == apiRetry.PredicateValueText:
		return strings.Compare(actual.Text(), expected.Text()), nil
	case actual.Tag() == apiRetry.PredicateValueBoolean && expected.Tag() == apiRetry.PredicateValueBoolean:
		return compareBool(actual.Boolean(), expected.Boolean()), nil
	case actual.Tag() == apiRetry.PredicateValueText && expected.Tag() == apiRetry.PredicateValueInteger:
		n, err := strconv.ParseInt(actual.Text(), 10, 64)
		if err != nil {
			return 0, errUnevaluable
		}
		return cmp.Compare(n, expected.Integer()), nil
	case actual.Tag() == apiRetry.PredicateValueInteger && expected.Tag() == apiRetry.PredicateValueText:
		return strings.Compare(strconv.FormatInt(actual.Integer(), 10), expected.Text()), nil
	default:
		return 0, errUnevaluable
	}
}

func compareBool(a, b bool) int {
	switch {
	case a == b:
		return 0
	case !a:
		return -1
	default:
		return 1
	}
}

func asText(v apiRetry.PredicateValue) (string, error) {
	switch v.Tag() {
	case apiRetry.PredicateValueText:
		return v.Text(), nil
	case apiRetry.PredicateValueInteger:
		return strconv.FormatInt(v.Integer(), 10), nil
	default:
		return "", errUnevaluable
	}
}

// ── Durations ────────────────────────────────────────────────────────────────

const maxDuration = time.Duration(math.MaxInt64)

// scaleDuration multiplies d by factor; a factor that is not a positive
// finite number gives zero, and a result too large for a Duration saturates.
func scaleDuration(d time.Duration, factor float64) time.Duration {
	if math.IsNaN(factor) || math.IsInf(factor, 0) || factor <= 0 {
		return 0
	}
	scaled := float64(d) * factor
	if math.IsNaN(scaled) || scaled >= math.MaxInt64 {
		return maxDuration
	}
	return time.Duration(math.Round(scaled))
}

func saturatingAdd(a, b time.Duration) time.Duration {
	if a > maxDuration-b {
		return maxDuration
	}
	return a + b
}

func saturatingInc(n uint32) uint32 {
	if n == math.MaxUint32 {
		return n
	}
	return n + 1
}
