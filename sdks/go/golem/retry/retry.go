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

// Package retry is a builder for Golem retry policies.
//
// Golem retries retriable operations (RPC, HTTP, RDBMS, and whole-invocation
// traps) according to named policy rules. Each rule is a [NamedPolicy]: a
// [Policy] (the retry strategy) plus a [Predicate] deciding when the rule
// applies and a priority. The runtime resolves, per operation, the
// highest-priority rule whose predicate matches, and applies its policy.
//
// Build a strategy from a base ([Exponential], [Periodic], [Fibonacci],
// [Immediate], [Never]) and refine it with fluent combinators:
//
//	pol := retry.Exponential(200*time.Millisecond, 2.0).
//		Clamp(100*time.Millisecond, 5*time.Second).
//		OnlyWhen(retry.StatusCode.OneOf(502, 503, 504)).
//		MaxRetries(3)
//	retry.Set(retry.Named("transient-http", pol).
//		WithPriority(10).
//		AppliesWhen(retry.Verb.Eq("GET")))
//
// A [Policy]/[Predicate]/[NamedPolicy] is an immutable value; combinators return
// a new value. The underlying wire form is an index-referenced node graph, but
// that is only ever produced by lowering — you compose values, never indices, so
// malformed graphs are unrepresentable. Value-range mistakes (a non-positive
// exponential factor, min > max in Clamp, a negative duration, an out-of-range
// integer, a zero-value tree) are reported: [Set] and [With] fail loud (panic)
// on an invalid policy, while [NamedPolicy.Validate] returns the error instead.
//
// # Where policies come from
//
// Base rules are usually declared in the application manifest (golem.yaml
// retryPolicyDefaults) or via the CLI/REST API; those are managed by the operator
// and stay live (the host refreshes them, like secrets), and this package reads
// them back with [GetPolicies], [GetByName], [Resolve] and [PolicyNames]. From
// agent code you overlay them: [Set] registers a rule that persists (and takes
// precedence over a live rule of the same name — use it when the code owns that
// rule), while [With] applies a rule only for the current call and restores the
// previous one on return. A common split: manifest for operator-tunable base
// rules, [Set] in the constructor for code-owned rules, [With] for per-call tweaks.
//
// # Concurrency
//
// [Set], [Remove] and [With] apply at the worker level — the retry scope is per
// worker, not per goroutine. Golem runs an agent single-threaded with cooperative
// task-switching only at await points (RPC, promise, sleep), so [With] is safe
// when its scope does not await while other goroutines run concurrently; nesting
// on a single logical flow is fine. To keep an override from affecting concurrent
// work, don't hold a scope open across a concurrent await (e.g. a CallAsync
// fan-out); for concurrency, distribute work across agent instances.
package retry

import (
	"errors"
	"fmt"
	"math"
	"time"

	apiRetry "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_retry"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// ── Policy: immutable retry-strategy tree ────────────────────────────────────

type policyKind uint8

const (
	pkPeriodic policyKind = iota
	pkExponential
	pkFibonacci
	pkImmediate
	pkNever
	pkCountBox
	pkTimeBox
	pkClampDelay
	pkAddDelay
	pkJitter
	pkFilteredOn
	pkAndThen
	pkUnion
	pkIntersect
)

// policyNode is the unexported tree node. Field use is per-kind (documented at
// each constructor); unused fields stay zero. Children are pointers so the graph
// is a value tree the user composes without ever seeing an index.
type policyNode struct {
	kind     policyKind
	d1       time.Duration // periodic delay | exponential base | fibonacci first | timeBox limit | clamp min | addDelay delay
	d2       time.Duration // fibonacci second | clamp max
	factor   float64       // exponential factor | jitter factor
	maxTries uint32        // countBox
	inner    *policyNode   // countBox/timeBox/clamp/addDelay/jitter/filteredOn child
	left     *policyNode   // andThen/union/intersect
	right    *policyNode
	pred     *predicateNode // filteredOn
}

// Policy is an immutable retry strategy. Its zero value is invalid; build one
// with a constructor.
type Policy struct{ node *policyNode }

// Periodic retries with a fixed delay between attempts.
func Periodic(d time.Duration) Policy {
	return Policy{&policyNode{kind: pkPeriodic, d1: d}}
}

// Exponential retries with an exponentially growing delay: base, base*factor,
// base*factor^2, … factor must be finite and > 0.
func Exponential(base time.Duration, factor float64) Policy {
	return Policy{&policyNode{kind: pkExponential, d1: base, factor: factor}}
}

// Fibonacci retries with a Fibonacci-growing delay seeded by first and second.
func Fibonacci(first, second time.Duration) Policy {
	return Policy{&policyNode{kind: pkFibonacci, d1: first, d2: second}}
}

// Immediate retries with no delay.
func Immediate() Policy { return Policy{&policyNode{kind: pkImmediate}} }

// Never does not retry.
func Never() Policy { return Policy{&policyNode{kind: pkNever}} }

// MaxRetries caps the number of retries.
func (p Policy) MaxRetries(n uint32) Policy {
	return Policy{&policyNode{kind: pkCountBox, maxTries: n, inner: p.node}}
}

// Within stops retrying once the given wall-clock budget has elapsed.
func (p Policy) Within(d time.Duration) Policy {
	return Policy{&policyNode{kind: pkTimeBox, d1: d, inner: p.node}}
}

// Clamp bounds each computed delay to [minDelay, maxDelay]. minDelay must be <= maxDelay.
func (p Policy) Clamp(minDelay, maxDelay time.Duration) Policy {
	return Policy{&policyNode{kind: pkClampDelay, d1: minDelay, d2: maxDelay, inner: p.node}}
}

// AddDelay adds a fixed delay to each computed delay.
func (p Policy) AddDelay(d time.Duration) Policy {
	return Policy{&policyNode{kind: pkAddDelay, d1: d, inner: p.node}}
}

// WithJitter randomizes each delay by up to the given factor. factor must be
// finite and >= 0.
func (p Policy) WithJitter(factor float64) Policy {
	return Policy{&policyNode{kind: pkJitter, factor: factor, inner: p.node}}
}

// OnlyWhen applies this strategy only when pred matches the operation context.
func (p Policy) OnlyWhen(pred Predicate) Policy {
	return Policy{&policyNode{kind: pkFilteredOn, pred: pred.node, inner: p.node}}
}

// AndThen falls back to next once this policy stops retrying.
func (p Policy) AndThen(next Policy) Policy {
	return Policy{&policyNode{kind: pkAndThen, left: p.node, right: next.node}}
}

// Union retries if either policy would retry (the shorter delay wins).
func (p Policy) Union(other Policy) Policy {
	return Policy{&policyNode{kind: pkUnion, left: p.node, right: other.node}}
}

// Intersect retries only if both policies would retry (the longer delay wins).
func (p Policy) Intersect(other Policy) Policy {
	return Policy{&policyNode{kind: pkIntersect, left: p.node, right: other.node}}
}

// ── Predicate: immutable match tree over the retry-context vocabulary ─────────

// PropName is a retry-context property. The exported constants are the platform
// vocabulary; use [Prop] for any other property name.
type PropName string

const (
	Verb              PropName = "verb"
	NounURI           PropName = "noun-uri"
	URIScheme         PropName = "uri-scheme"
	URIHost           PropName = "uri-host"
	URIPort           PropName = "uri-port"
	URIPath           PropName = "uri-path"
	StatusCode        PropName = "status-code"
	ErrorType         PropName = "error-type"
	Function          PropName = "function"
	TargetComponentID PropName = "target-component-id"
	TargetAgentType   PropName = "target-agent-type"
	DBType            PropName = "db-type"
	TrapType          PropName = "trap-type"
)

// Prop names a custom retry-context property not covered by the constants above.
func Prop(name string) PropName { return PropName(name) }

type predKind uint8

const (
	prEq predKind = iota
	prNeq
	prGt
	prGte
	prLt
	prLte
	prExists
	prIn
	prMatches
	prStartsWith
	prContains
	prAnd
	prOr
	prNot
	prTrue
	prFalse
)

type predicateNode struct {
	kind  predKind
	prop  string // property name (comparisons/exists/in/patterns)
	val   any    // comparison value (eq/neq/gt/gte/lt/lte)
	vals  []any  // OneOf values
	pat   string // matches/startsWith/contains
	left  *predicateNode
	right *predicateNode
	inner *predicateNode // Not
}

// Predicate is an immutable boolean match over the operation context. Its zero
// value is invalid; build one from a [PropName] method or [MatchAlways]/
// [MatchNever]. Predicate values may be string, any integer type, or bool.
type Predicate struct{ node *predicateNode }

func (p PropName) cmp(kind predKind, v any) Predicate {
	return Predicate{&predicateNode{kind: kind, prop: string(p), val: v}}
}

// Eq matches when the property equals v.
func (p PropName) Eq(v any) Predicate { return p.cmp(prEq, v) }

// Neq matches when the property does not equal v.
func (p PropName) Neq(v any) Predicate { return p.cmp(prNeq, v) }

// Gt matches when the property is greater than v.
func (p PropName) Gt(v any) Predicate { return p.cmp(prGt, v) }

// Gte matches when the property is greater than or equal to v.
func (p PropName) Gte(v any) Predicate { return p.cmp(prGte, v) }

// Lt matches when the property is less than v.
func (p PropName) Lt(v any) Predicate { return p.cmp(prLt, v) }

// Lte matches when the property is less than or equal to v.
func (p PropName) Lte(v any) Predicate { return p.cmp(prLte, v) }

// Exists matches when the property is present in the context.
func (p PropName) Exists() Predicate {
	return Predicate{&predicateNode{kind: prExists, prop: string(p)}}
}

// OneOf matches when the property equals any of the given values.
func (p PropName) OneOf(vs ...any) Predicate {
	return Predicate{&predicateNode{kind: prIn, prop: string(p), vals: vs}}
}

// MatchesGlob matches when the property matches the glob pattern.
func (p PropName) MatchesGlob(pattern string) Predicate {
	return Predicate{&predicateNode{kind: prMatches, prop: string(p), pat: pattern}}
}

// StartsWith matches when the property starts with prefix.
func (p PropName) StartsWith(prefix string) Predicate {
	return Predicate{&predicateNode{kind: prStartsWith, prop: string(p), pat: prefix}}
}

// Contains matches when the property contains sub.
func (p PropName) Contains(sub string) Predicate {
	return Predicate{&predicateNode{kind: prContains, prop: string(p), pat: sub}}
}

// And matches when both predicates match.
func (p Predicate) And(q Predicate) Predicate {
	return Predicate{&predicateNode{kind: prAnd, left: p.node, right: q.node}}
}

// Or matches when either predicate matches.
func (p Predicate) Or(q Predicate) Predicate {
	return Predicate{&predicateNode{kind: prOr, left: p.node, right: q.node}}
}

// Not negates the predicate.
func (p Predicate) Not() Predicate {
	return Predicate{&predicateNode{kind: prNot, inner: p.node}}
}

// MatchAlways always matches (the default applicability of a [NamedPolicy]).
func MatchAlways() Predicate { return Predicate{&predicateNode{kind: prTrue}} }

// MatchNever never matches.
func MatchNever() Predicate { return Predicate{&predicateNode{kind: prFalse}} }

// ── NamedPolicy ──────────────────────────────────────────────────────────────

// NamedPolicy is a retry rule: a [Policy], the [Predicate] that selects when it
// applies, and a priority (higher is checked first). Build with [Named].
type NamedPolicy struct {
	name     string
	priority uint32
	applies  Predicate // zero => MatchAlways
	policy   Policy
}

// Named creates a rule with priority 0 that always applies.
func Named(name string, policy Policy) NamedPolicy {
	return NamedPolicy{name: name, policy: policy}
}

// WithPriority sets the evaluation priority (higher is checked first).
func (n NamedPolicy) WithPriority(p uint32) NamedPolicy { n.priority = p; return n }

// AppliesWhen restricts the rule to contexts matching pred.
func (n NamedPolicy) AppliesWhen(pred Predicate) NamedPolicy { n.applies = pred; return n }

// Name returns the rule name.
func (n NamedPolicy) Name() string { return n.name }

// Priority returns the evaluation priority.
func (n NamedPolicy) Priority() uint32 { return n.priority }

// Strategy returns the retry [Policy] of the rule. The returned [Policy] is
// opaque — re-apply or re-inspect via [Set]/[NamedPolicy.Validate], not
// field-by-field.
func (n NamedPolicy) Strategy() Policy { return n.policy }

// Applicability returns the [Predicate] selecting when the rule applies
// ([MatchAlways] if none was set).
func (n NamedPolicy) Applicability() Predicate {
	if n.applies.node == nil {
		return MatchAlways()
	}
	return n.applies
}

// Validate reports whether the rule lowers to a valid wire policy, returning the
// first problem found (nil if valid). It is the non-panicking counterpart to the
// checks [Set] and [With] perform.
func (n NamedPolicy) Validate() error {
	if _, err := n.lower(); err != nil {
		return fmt.Errorf("golem/retry: %w", err)
	}
	return nil
}

// ── Apply / scope (worker-global; see the package "Concurrency" note) ─────────

// Set registers or overwrites the named rule and PERSISTS it for this agent (to
// the oplog) — it stays in effect across invocations and takes precedence over a
// manifest/CLI rule of the same name (so that name no longer tracks live updates).
// Reach for it when the code owns the rule; prefer the manifest for operator-
// tunable base rules, and [With] for a change scoped to one call. Panics if the
// policy is invalid; use [NamedPolicy.Validate] to check first.
func Set(n NamedPolicy) {
	raw, err := n.lower()
	if err != nil {
		panic(fmt.Errorf("golem/retry: %w", err))
	}
	apiRetry.SetRetryPolicy(raw)
}

// Remove deletes the named rule.
func Remove(name string) { apiRetry.RemoveRetryPolicy(name) }

// With applies the rule for the current call only and returns a function that
// restores the previously registered rule of the same name (or removes it if
// there was none). Use it with defer to scope a per-invocation override:
// defer retry.With(n)(). Because invocations run sequentially, the override does
// not affect the next invocation. Panics if invalid. See the package "Concurrency"
// note before using it across an await.
func With(n NamedPolicy) (restore func()) {
	raw, err := n.lower()
	if err != nil {
		panic(fmt.Errorf("golem/retry: %w", err))
	}
	name := n.name
	prev := apiRetry.GetRetryPolicyByName(name)
	apiRetry.SetRetryPolicy(raw)
	return func() {
		if prev.IsSome() {
			apiRetry.SetRetryPolicy(prev.Some())
		} else {
			apiRetry.RemoveRetryPolicy(name)
		}
	}
}

// PolicyNames returns the names of the rules active for this agent (the built-in
// "default" plus any registered). Cheap: it does not decode the policy bodies.
func PolicyNames() []string {
	raw := apiRetry.GetRetryPolicies()
	names := make([]string, len(raw))
	for i, p := range raw {
		names[i] = p.Name
	}
	return names
}

// GetPolicies returns the rules active for this agent, decoded into [NamedPolicy]
// values. It errors if the host returned a malformed policy graph.
func GetPolicies() ([]NamedPolicy, error) {
	raw := apiRetry.GetRetryPolicies()
	out := make([]NamedPolicy, 0, len(raw))
	for _, r := range raw {
		np, err := decodeNamed(r)
		if err != nil {
			return nil, err
		}
		out = append(out, np)
	}
	return out, nil
}

// GetByName returns the rule with the given name. found is false if no such rule
// exists; err is non-nil only if the host returned a malformed policy graph.
func GetByName(name string) (policy NamedPolicy, found bool, err error) {
	opt := apiRetry.GetRetryPolicyByName(name)
	if opt.IsNone() {
		return NamedPolicy{}, false, nil
	}
	np, err := decodeNamed(opt.Some())
	if err != nil {
		return NamedPolicy{}, true, err
	}
	return np, true, nil
}

// Resolve returns the [Policy] the runtime would apply to an operation with the
// given verb, noun URI and context properties (the highest-priority matching
// rule). matched is false if no rule matches.
func Resolve(verb, nounURI string, props map[string]any) (policy Policy, matched bool, err error) {
	tuples := make([]witTypes.Tuple2[string, apiRetry.PredicateValue], 0, len(props))
	for k, v := range props {
		pv, cErr := toPredicateValue(v)
		if cErr != nil {
			return Policy{}, false, cErr
		}
		tuples = append(tuples, witTypes.Tuple2[string, apiRetry.PredicateValue]{F0: k, F1: pv})
	}
	opt := apiRetry.ResolveRetryPolicy(verb, nounURI, tuples)
	if opt.IsNone() {
		return Policy{}, false, nil
	}
	pol, dErr := decodePolicy(opt.Some())
	if dErr != nil {
		return Policy{}, true, dErr
	}
	return pol, true, nil
}

// ── Lowering (value tree -> flattened index graph); pure, no host calls ───────

func (n NamedPolicy) lower() (apiRetry.NamedRetryPolicy, error) {
	pol, err := n.policy.lower()
	if err != nil {
		return apiRetry.NamedRetryPolicy{}, err
	}
	applies := n.applies
	if applies.node == nil {
		applies = MatchAlways()
	}
	pred, err := applies.lower()
	if err != nil {
		return apiRetry.NamedRetryPolicy{}, err
	}
	return apiRetry.NamedRetryPolicy{Name: n.name, Priority: n.priority, Predicate: pred, Policy: pol}, nil
}

func (p Policy) lower() (apiRetry.RetryPolicy, error) {
	if p.node == nil {
		return apiRetry.RetryPolicy{}, errors.New("invalid (zero) Policy; build one with a constructor such as retry.Exponential or retry.Periodic")
	}
	var nodes []apiRetry.PolicyNode
	if _, err := pushPolicy(p.node, &nodes); err != nil {
		return apiRetry.RetryPolicy{}, err
	}
	return apiRetry.RetryPolicy{Nodes: nodes}, nil
}

func pushPolicy(n *policyNode, nodes *[]apiRetry.PolicyNode) (int32, error) {
	if n == nil {
		return 0, errors.New("incomplete Policy: a combinator was applied to a zero Policy")
	}
	idx := int32(len(*nodes))
	*nodes = append(*nodes, apiRetry.MakePolicyNodeNever()) // reserve slot; backfilled below

	var out apiRetry.PolicyNode
	switch n.kind {
	case pkPeriodic:
		if err := checkDur("Periodic delay", n.d1); err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodePeriodic(ns(n.d1))
	case pkExponential:
		if err := checkDur("Exponential base", n.d1); err != nil {
			return 0, err
		}
		if !finitePositive(n.factor) {
			return 0, fmt.Errorf("Exponential factor must be finite and > 0, got %v", n.factor)
		}
		out = apiRetry.MakePolicyNodeExponential(apiRetry.ExponentialConfig{BaseDelay: ns(n.d1), Factor: n.factor})
	case pkFibonacci:
		if err := checkDur("Fibonacci first", n.d1); err != nil {
			return 0, err
		}
		if err := checkDur("Fibonacci second", n.d2); err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeFibonacci(apiRetry.FibonacciConfig{First: ns(n.d1), Second: ns(n.d2)})
	case pkImmediate:
		out = apiRetry.MakePolicyNodeImmediate()
	case pkNever:
		out = apiRetry.MakePolicyNodeNever()
	case pkCountBox:
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeCountBox(apiRetry.CountBoxConfig{MaxRetries: n.maxTries, Inner: inner})
	case pkTimeBox:
		if err := checkDur("Within budget", n.d1); err != nil {
			return 0, err
		}
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeTimeBox(apiRetry.TimeBoxConfig{Limit: ns(n.d1), Inner: inner})
	case pkClampDelay:
		if err := checkDur("Clamp min", n.d1); err != nil {
			return 0, err
		}
		if err := checkDur("Clamp max", n.d2); err != nil {
			return 0, err
		}
		if n.d1 > n.d2 {
			return 0, fmt.Errorf("Clamp min (%v) must be <= max (%v)", n.d1, n.d2)
		}
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeClampDelay(apiRetry.ClampConfig{MinDelay: ns(n.d1), MaxDelay: ns(n.d2), Inner: inner})
	case pkAddDelay:
		if err := checkDur("AddDelay", n.d1); err != nil {
			return 0, err
		}
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeAddDelay(apiRetry.AddDelayConfig{Delay: ns(n.d1), Inner: inner})
	case pkJitter:
		if !finiteNonNegative(n.factor) {
			return 0, fmt.Errorf("WithJitter factor must be finite and >= 0, got %v", n.factor)
		}
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeJitter(apiRetry.JitterConfig{Factor: n.factor, Inner: inner})
	case pkFilteredOn:
		pred, err := (Predicate{n.pred}).lower()
		if err != nil {
			return 0, err
		}
		inner, err := pushPolicy(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePolicyNodeFilteredOn(apiRetry.FilteredConfig{Predicate: pred, Inner: inner})
	case pkAndThen, pkUnion, pkIntersect:
		left, err := pushPolicy(n.left, nodes)
		if err != nil {
			return 0, err
		}
		right, err := pushPolicy(n.right, nodes)
		if err != nil {
			return 0, err
		}
		pair := witTypes.Tuple2[int32, int32]{F0: left, F1: right}
		switch n.kind {
		case pkAndThen:
			out = apiRetry.MakePolicyNodeAndThen(pair)
		case pkUnion:
			out = apiRetry.MakePolicyNodePolicyUnion(pair)
		default:
			out = apiRetry.MakePolicyNodePolicyIntersect(pair)
		}
	default:
		return 0, fmt.Errorf("internal: unknown policy kind %d", n.kind)
	}

	(*nodes)[idx] = out
	return idx, nil
}

func (p Predicate) lower() (apiRetry.RetryPredicate, error) {
	if p.node == nil {
		return apiRetry.RetryPredicate{}, errors.New("invalid (zero) Predicate; build one from a retry property method or retry.MatchAlways")
	}
	var nodes []apiRetry.PredicateNode
	if _, err := pushPredicate(p.node, &nodes); err != nil {
		return apiRetry.RetryPredicate{}, err
	}
	return apiRetry.RetryPredicate{Nodes: nodes}, nil
}

func pushPredicate(n *predicateNode, nodes *[]apiRetry.PredicateNode) (int32, error) {
	if n == nil {
		return 0, errors.New("incomplete Predicate: a combinator was applied to a zero Predicate")
	}
	idx := int32(len(*nodes))
	*nodes = append(*nodes, apiRetry.MakePredicateNodePredFalse()) // reserve slot

	comparison := func() (apiRetry.PropertyComparison, error) {
		pv, err := toPredicateValue(n.val)
		return apiRetry.PropertyComparison{PropertyName: n.prop, Value: pv}, err
	}

	var out apiRetry.PredicateNode
	switch n.kind {
	case prEq, prNeq, prGt, prGte, prLt, prLte:
		c, err := comparison()
		if err != nil {
			return 0, err
		}
		switch n.kind {
		case prEq:
			out = apiRetry.MakePredicateNodePropEq(c)
		case prNeq:
			out = apiRetry.MakePredicateNodePropNeq(c)
		case prGt:
			out = apiRetry.MakePredicateNodePropGt(c)
		case prGte:
			out = apiRetry.MakePredicateNodePropGte(c)
		case prLt:
			out = apiRetry.MakePredicateNodePropLt(c)
		default:
			out = apiRetry.MakePredicateNodePropLte(c)
		}
	case prExists:
		out = apiRetry.MakePredicateNodePropExists(n.prop)
	case prIn:
		vals := make([]apiRetry.PredicateValue, 0, len(n.vals))
		for _, v := range n.vals {
			pv, err := toPredicateValue(v)
			if err != nil {
				return 0, err
			}
			vals = append(vals, pv)
		}
		out = apiRetry.MakePredicateNodePropIn(apiRetry.PropertySetCheck{PropertyName: n.prop, Values: vals})
	case prMatches:
		out = apiRetry.MakePredicateNodePropMatches(apiRetry.PropertyPattern{PropertyName: n.prop, Pattern: n.pat})
	case prStartsWith:
		out = apiRetry.MakePredicateNodePropStartsWith(apiRetry.PropertyPattern{PropertyName: n.prop, Pattern: n.pat})
	case prContains:
		out = apiRetry.MakePredicateNodePropContains(apiRetry.PropertyPattern{PropertyName: n.prop, Pattern: n.pat})
	case prAnd, prOr:
		left, err := pushPredicate(n.left, nodes)
		if err != nil {
			return 0, err
		}
		right, err := pushPredicate(n.right, nodes)
		if err != nil {
			return 0, err
		}
		pair := witTypes.Tuple2[int32, int32]{F0: left, F1: right}
		if n.kind == prAnd {
			out = apiRetry.MakePredicateNodePredAnd(pair)
		} else {
			out = apiRetry.MakePredicateNodePredOr(pair)
		}
	case prNot:
		inner, err := pushPredicate(n.inner, nodes)
		if err != nil {
			return 0, err
		}
		out = apiRetry.MakePredicateNodePredNot(inner)
	case prTrue:
		out = apiRetry.MakePredicateNodePredTrue()
	case prFalse:
		out = apiRetry.MakePredicateNodePredFalse()
	default:
		return 0, fmt.Errorf("internal: unknown predicate kind %d", n.kind)
	}

	(*nodes)[idx] = out
	return idx, nil
}

// ── Value coercion + range/finiteness checks ─────────────────────────────────

func toPredicateValue(v any) (apiRetry.PredicateValue, error) {
	switch x := v.(type) {
	case string:
		return apiRetry.MakePredicateValueText(x), nil
	case bool:
		return apiRetry.MakePredicateValueBoolean(x), nil
	case int:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case int8:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case int16:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case int32:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case int64:
		return apiRetry.MakePredicateValueInteger(x), nil
	case uint8:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case uint16:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case uint32:
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case uint:
		if uint64(x) > math.MaxInt64 {
			return apiRetry.PredicateValue{}, fmt.Errorf("predicate integer %d overflows int64", x)
		}
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	case uint64:
		if x > math.MaxInt64 {
			return apiRetry.PredicateValue{}, fmt.Errorf("predicate integer %d overflows int64", x)
		}
		return apiRetry.MakePredicateValueInteger(int64(x)), nil
	default:
		return apiRetry.PredicateValue{}, fmt.Errorf("unsupported predicate value type %T (want string, an integer type, or bool)", v)
	}
}

func checkDur(what string, d time.Duration) error {
	if d < 0 {
		return fmt.Errorf("%s must be non-negative, got %v", what, d)
	}
	return nil
}

func ns(d time.Duration) uint64 { return uint64(d.Nanoseconds()) }

func finitePositive(f float64) bool {
	return !math.IsNaN(f) && !math.IsInf(f, 0) && f > 0
}

func finiteNonNegative(f float64) bool {
	return !math.IsNaN(f) && !math.IsInf(f, 0) && f >= 0
}

// ── Decoding (host graph -> value tree); guarded against malformed input ──────

func decodeNamed(raw apiRetry.NamedRetryPolicy) (NamedPolicy, error) {
	pol, err := decodePolicy(raw.Policy)
	if err != nil {
		return NamedPolicy{}, err
	}
	pred, err := decodePredicate(raw.Predicate)
	if err != nil {
		return NamedPolicy{}, err
	}
	return NamedPolicy{name: raw.Name, priority: raw.Priority, applies: pred, policy: pol}, nil
}

func decodePolicy(raw apiRetry.RetryPolicy) (Policy, error) {
	node, err := decodePolicyNode(raw.Nodes, 0, map[int32]bool{})
	if err != nil {
		return Policy{}, err
	}
	return Policy{node}, nil
}

func decodePolicyNode(nodes []apiRetry.PolicyNode, idx int32, path map[int32]bool) (*policyNode, error) {
	if len(nodes) == 0 {
		return nil, errors.New("golem/retry: empty policy node list")
	}
	if idx < 0 || int(idx) >= len(nodes) {
		return nil, fmt.Errorf("golem/retry: policy node index %d out of range [0,%d)", idx, len(nodes))
	}
	if path[idx] {
		return nil, fmt.Errorf("golem/retry: cycle in policy graph at node %d", idx)
	}
	path[idx] = true
	defer delete(path, idx)

	w := nodes[idx]
	switch w.Tag() {
	case apiRetry.PolicyNodePeriodic:
		return &policyNode{kind: pkPeriodic, d1: durOf(w.Periodic())}, nil
	case apiRetry.PolicyNodeExponential:
		c := w.Exponential()
		return &policyNode{kind: pkExponential, d1: durOf(c.BaseDelay), factor: c.Factor}, nil
	case apiRetry.PolicyNodeFibonacci:
		c := w.Fibonacci()
		return &policyNode{kind: pkFibonacci, d1: durOf(c.First), d2: durOf(c.Second)}, nil
	case apiRetry.PolicyNodeImmediate:
		return &policyNode{kind: pkImmediate}, nil
	case apiRetry.PolicyNodeNever:
		return &policyNode{kind: pkNever}, nil
	case apiRetry.PolicyNodeCountBox:
		c := w.CountBox()
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkCountBox, maxTries: c.MaxRetries, inner: inner}, nil
	case apiRetry.PolicyNodeTimeBox:
		c := w.TimeBox()
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkTimeBox, d1: durOf(c.Limit), inner: inner}, nil
	case apiRetry.PolicyNodeClampDelay:
		c := w.ClampDelay()
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkClampDelay, d1: durOf(c.MinDelay), d2: durOf(c.MaxDelay), inner: inner}, nil
	case apiRetry.PolicyNodeAddDelay:
		c := w.AddDelay()
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkAddDelay, d1: durOf(c.Delay), inner: inner}, nil
	case apiRetry.PolicyNodeJitter:
		c := w.Jitter()
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkJitter, factor: c.Factor, inner: inner}, nil
	case apiRetry.PolicyNodeFilteredOn:
		c := w.FilteredOn()
		pred, err := decodePredicate(c.Predicate)
		if err != nil {
			return nil, err
		}
		inner, err := decodePolicyNode(nodes, c.Inner, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: pkFilteredOn, pred: pred.node, inner: inner}, nil
	case apiRetry.PolicyNodeAndThen, apiRetry.PolicyNodePolicyUnion, apiRetry.PolicyNodePolicyIntersect:
		var pair witTypes.Tuple2[int32, int32]
		var kind policyKind
		switch w.Tag() {
		case apiRetry.PolicyNodeAndThen:
			pair, kind = w.AndThen(), pkAndThen
		case apiRetry.PolicyNodePolicyUnion:
			pair, kind = w.PolicyUnion(), pkUnion
		default:
			pair, kind = w.PolicyIntersect(), pkIntersect
		}
		left, err := decodePolicyNode(nodes, pair.F0, path)
		if err != nil {
			return nil, err
		}
		right, err := decodePolicyNode(nodes, pair.F1, path)
		if err != nil {
			return nil, err
		}
		return &policyNode{kind: kind, left: left, right: right}, nil
	default:
		return nil, fmt.Errorf("golem/retry: unknown policy node tag %d", w.Tag())
	}
}

func decodePredicate(raw apiRetry.RetryPredicate) (Predicate, error) {
	node, err := decodePredicateNode(raw.Nodes, 0, map[int32]bool{})
	if err != nil {
		return Predicate{}, err
	}
	return Predicate{node}, nil
}

func decodePredicateNode(nodes []apiRetry.PredicateNode, idx int32, path map[int32]bool) (*predicateNode, error) {
	if len(nodes) == 0 {
		return nil, errors.New("golem/retry: empty predicate node list")
	}
	if idx < 0 || int(idx) >= len(nodes) {
		return nil, fmt.Errorf("golem/retry: predicate node index %d out of range [0,%d)", idx, len(nodes))
	}
	if path[idx] {
		return nil, fmt.Errorf("golem/retry: cycle in predicate graph at node %d", idx)
	}
	path[idx] = true
	defer delete(path, idx)

	w := nodes[idx]
	cmp := func(kind predKind, c apiRetry.PropertyComparison) (*predicateNode, error) {
		return &predicateNode{kind: kind, prop: c.PropertyName, val: fromPredicateValue(c.Value)}, nil
	}
	switch w.Tag() {
	case apiRetry.PredicateNodePropEq:
		return cmp(prEq, w.PropEq())
	case apiRetry.PredicateNodePropNeq:
		return cmp(prNeq, w.PropNeq())
	case apiRetry.PredicateNodePropGt:
		return cmp(prGt, w.PropGt())
	case apiRetry.PredicateNodePropGte:
		return cmp(prGte, w.PropGte())
	case apiRetry.PredicateNodePropLt:
		return cmp(prLt, w.PropLt())
	case apiRetry.PredicateNodePropLte:
		return cmp(prLte, w.PropLte())
	case apiRetry.PredicateNodePropExists:
		return &predicateNode{kind: prExists, prop: w.PropExists()}, nil
	case apiRetry.PredicateNodePropIn:
		c := w.PropIn()
		vals := make([]any, 0, len(c.Values))
		for _, pv := range c.Values {
			vals = append(vals, fromPredicateValue(pv))
		}
		return &predicateNode{kind: prIn, prop: c.PropertyName, vals: vals}, nil
	case apiRetry.PredicateNodePropMatches:
		c := w.PropMatches()
		return &predicateNode{kind: prMatches, prop: c.PropertyName, pat: c.Pattern}, nil
	case apiRetry.PredicateNodePropStartsWith:
		c := w.PropStartsWith()
		return &predicateNode{kind: prStartsWith, prop: c.PropertyName, pat: c.Pattern}, nil
	case apiRetry.PredicateNodePropContains:
		c := w.PropContains()
		return &predicateNode{kind: prContains, prop: c.PropertyName, pat: c.Pattern}, nil
	case apiRetry.PredicateNodePredAnd, apiRetry.PredicateNodePredOr:
		var pair witTypes.Tuple2[int32, int32]
		var kind predKind
		if w.Tag() == apiRetry.PredicateNodePredAnd {
			pair, kind = w.PredAnd(), prAnd
		} else {
			pair, kind = w.PredOr(), prOr
		}
		left, err := decodePredicateNode(nodes, pair.F0, path)
		if err != nil {
			return nil, err
		}
		right, err := decodePredicateNode(nodes, pair.F1, path)
		if err != nil {
			return nil, err
		}
		return &predicateNode{kind: kind, left: left, right: right}, nil
	case apiRetry.PredicateNodePredNot:
		inner, err := decodePredicateNode(nodes, w.PredNot(), path)
		if err != nil {
			return nil, err
		}
		return &predicateNode{kind: prNot, inner: inner}, nil
	case apiRetry.PredicateNodePredTrue:
		return &predicateNode{kind: prTrue}, nil
	case apiRetry.PredicateNodePredFalse:
		return &predicateNode{kind: prFalse}, nil
	default:
		return nil, fmt.Errorf("golem/retry: unknown predicate node tag %d", w.Tag())
	}
}

func fromPredicateValue(pv apiRetry.PredicateValue) any {
	switch pv.Tag() {
	case apiRetry.PredicateValueText:
		return pv.Text()
	case apiRetry.PredicateValueInteger:
		return pv.Integer()
	default:
		return pv.Boolean()
	}
}

func durOf(nanos uint64) time.Duration { return time.Duration(nanos) }
