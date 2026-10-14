// Package identity is the Go self-identity fixture driven by self_identity.rs.
package identity

import (
	"encoding/json"
	"strconv"
	"strings"

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
	saved.SnapshotIdentity = golem.MustGetSelfMetadata().AgentID.AgentID
	return json.Marshal(saved)
}

func (s *state) Load(b []byte) error { return json.Unmarshal(b, s) }

type ForkNamedIn struct {
	Name   string
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
	checkpoint := def.Method[golem.Unit, string]("checkpoint")
	forkNamed := def.Method[ForkNamedIn, golem.Unit]("forkNamed")
	forkSelf := def.Method[golem.Unit, string]("forkSelf")

	impl := def.ImplementConfigured(func(ic *golem.InitContext[Id, state, golem.NoConfig]) *state {
		return &state{Configured: struct{ Name string }(ic.ID()).Name, Saved: ic.AgentID(), Trace: []string{}}
	})
	impl.Handle(observe, func(ctx *golem.Context[state], _ golem.Unit) string {
		self := ctx.AgentID()
		// Different durable calls make an incorrect historical identity diverge.
		if strings.Contains(self, `"parent"`) {
			golem.MustGetSelfMetadata()
		} else {
			durability.OplogIndex()
		}
		ctx.State.Trace = append(ctx.State.Trace, self)
		return self
	})
	impl.Handle(status, func(ctx *golem.Context[state], _ golem.Unit) string {
		current := ctx.AgentID()
		metadata := golem.MustGetSelfMetadata()
		addressed, _ := golem.GetAgentMetadata(golem.AgentID{ComponentID: metadata.AgentID.ComponentID, AgentID: current})
		out, _ := json.Marshal(map[string]any{
			"configured":       ctx.State.Configured,
			"saved":            ctx.State.Saved,
			"trace":            ctx.State.Trace,
			"snapshotIdentity": ctx.State.SnapshotIdentity,
			"lastFork":         ctx.State.LastFork,
			"current":          current,
			"metadata":         metadata.AgentID.AgentID,
			"addressed":        addressed.AgentID.AgentID,
		})
		return string(out)
	})
	impl.Handle(checkpoint, func(*golem.Context[state], golem.Unit) string {
		return strconv.FormatUint(durability.OplogIndex(), 10)
	})
	impl.Handle(forkNamed, func(ctx *golem.Context[state], in ForkNamedIn) golem.Unit {
		cutoff := golem.Must(strconv.ParseUint(in.Cutoff, 10, 64))
		source := golem.AgentID{ComponentID: golem.MustGetSelfMetadata().AgentID.ComponentID, AgentID: ctx.AgentID()}
		target := golem.AgentID{ComponentID: source.ComponentID, AgentID: def.AgentID(Id(struct{ Name string }{in.Name}), golem.None[golem.UUID]())}
		golem.MustForkAgent(source, target, cutoff)
		return golem.Unit{}
	})
	impl.Handle(forkSelf, func(ctx *golem.Context[state], _ golem.Unit) string {
		before := ctx.AgentID()
		forked, phantom := golem.MustForkSelf()
		after := ctx.AgentID()
		parsed := golem.Must(golem.ParseAgentID[Id](before))
		tag := "original"
		if forked {
			tag = "forked"
		}
		out, _ := json.Marshal(map[string]any{
			"before": before,
			"after":  after,
			"tag":    tag,
			"target": def.AgentID(parsed.ID, golem.Some(phantom)),
		})
		ctx.State.LastFork = string(out)
		return ctx.State.LastFork
	})
}
