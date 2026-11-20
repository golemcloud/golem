// Package identity is the Go self-identity fixture driven by self_identity.rs.
package identity

import (
	"encoding/json"
	"strconv"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durability"
)

// Each agent type needs its own Id type.
type (
	ID         struct{ Name string }
	SnapshotID struct{ Name string }
)

type named interface{ ~struct{ Name string } }

type state struct {
	Configured       string
	Saved            string
	Trace            []string
	SnapshotIdentity string
	LastFork         string
}

// Save records the identity of the agent being snapshotted, so a restore
// carries the saving agent's id rather than reading its own.
func (s *state) Save() ([]byte, error) {
	saved := *s
	saved.SnapshotIdentity = golem.MustGetSelfMetadata().AgentID.String()
	return json.Marshal(saved)
}

func (s *state) Load(b []byte) error { return json.Unmarshal(b, s) }

type ForkAtIn struct {
	Cutoff string
}

func init() {
	define[ID]("Identity", golem.SnapshotDefault)
	define[SnapshotID]("SnapshotIdentity", golem.SnapshotEveryN(2))
}

func define[Id named](name string, snapshot golem.SnapshotPolicy) {
	def := golem.DefineAgent[Id](golem.Spec{Name: name, Snapshot: snapshot})
	observe := def.Method[golem.Unit, string]("observe")
	status := def.Method[golem.Unit, string]("status")
	forkAt := def.Method[ForkAtIn, string]("forkAt")
	forkSelf := def.Method[golem.Unit, string]("forkSelf")

	impl := def.ImplementConfigured(func(ic *golem.InitContext[Id, state, golem.NoConfig]) *state {
		return &state{Configured: struct{ Name string }(ic.ID()).Name, Saved: ic.AgentID().String(), Trace: []string{}}
	})
	impl.Handle(observe, func(ctx *golem.Context[state], _ golem.Unit) string {
		self := ctx.AgentID().String()
		// Different durable calls make an incorrect historical identity diverge.
		if self == ctx.State.Saved {
			golem.MustGetSelfMetadata()
		} else {
			durability.GenerateIdempotencyKey()
		}
		ctx.State.Trace = append(ctx.State.Trace, self)
		return self
	})
	impl.Handle(status, func(ctx *golem.Context[state], _ golem.Unit) string {
		current := ctx.AgentID()
		metadata := golem.MustGetSelfMetadata()
		addressed, _ := golem.GetAgentMetadata(current)
		out, _ := json.Marshal(map[string]any{
			"configured":       ctx.State.Configured,
			"saved":            ctx.State.Saved,
			"trace":            ctx.State.Trace,
			"snapshotIdentity": ctx.State.SnapshotIdentity,
			"lastFork":         ctx.State.LastFork,
			"current":          current.String(),
			"metadata":         metadata.AgentID.String(),
			"addressed":        addressed.AgentID.String(),
		})
		return string(out)
	})
	impl.Handle(forkAt, func(ctx *golem.Context[state], in ForkAtIn) string {
		cutoff := golem.Must(strconv.ParseUint(in.Cutoff, 10, 64))
		return golem.MustForkAgent(ctx.AgentID(), cutoff).String()
	})
	impl.Handle(forkSelf, func(ctx *golem.Context[state], _ golem.Unit) string {
		before := ctx.AgentID().String()
		forked, forkedID := golem.MustForkSelf()
		after := ctx.AgentID().String()
		tag := "original"
		if forked {
			tag = "forked"
		}
		out, _ := json.Marshal(map[string]any{
			"before": before,
			"after":  after,
			"tag":    tag,
			"target": forkedID.String(),
		})
		ctx.State.LastFork = string(out)
		return ctx.State.LastFork
	})
}
