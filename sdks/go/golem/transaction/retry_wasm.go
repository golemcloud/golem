//go:build wasip1

package transaction

import (
	"math"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
	api "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_api_retry"
	clock "github.com/golemcloud/golem/sdks/go/golem/internal/wit/wasi_clocks_0_3_0_monotonic_clock"
	wt "go.bytecodealliance.org/pkg/wit/types"
)

var sagaNow = clock.Now

var sagaNext = func(state []byte, elapsed uint64, attempt int64, kind string) ([]byte, time.Duration, bool, *PolicyError) {
	meta, err := golem.GetSelfMetadata()
	if err != nil {
		return nil, 0, false, &PolicyError{Message: err.Error()}
	}
	uri := "urn:worker:" + meta.AgentID.ComponentID.String() + "/" + meta.AgentID.AgentID
	props := []wt.Tuple2[string, api.PredicateValue]{
		{F0: "attempt", F1: api.MakePredicateValueInteger(attempt)},
		{F0: "error-kind", F1: api.MakePredicateValueText(kind)},
	}
	policy := api.ResolveRetryPolicy("saga", uri, props)
	if policy.IsErr() {
		return nil, 0, false, &PolicyError{Message: policy.Err()}
	}
	r := api.NextRetryDelay(policy.Ok(), state, elapsed, "saga", uri, props)
	if r.IsErr() {
		return nil, 0, false, &PolicyError{Message: r.Err()}
	}
	d := r.Ok()
	if d.Delay.IsNone() {
		return d.State, 0, false, nil
	}
	if d.Delay.Some() > math.MaxInt64 {
		return d.State, 0, false, &PolicyError{Message: "retry delay exceeds Go duration range"}
	}
	return d.State, time.Duration(d.Delay.Some()), true, nil
}
