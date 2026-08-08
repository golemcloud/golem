// Session agent in Go. It shows how to control state snapshotting: the SDK asks
// the guest to serialize durable state periodically (and on upgrade), so custom
// serialization is worth demonstrating alongside the default.
package session

import "github.com/golemcloud/golem/sdks/go/golem"

// SessionID identifies the session agent by its constructor parameters.
type SessionID struct{ User string }

// SessionState keeps a running total in an unexported field. Reflection cannot
// see unexported fields, so the default (JSON of exported fields) would drop it;
// implementing Snapshotter below is what makes the state survive a snapshot.
type SessionState struct{ total int64 }

// Save/Load make SessionState a golem.Snapshotter: the returned bytes are stored
// verbatim by the host and handed back on restore, so unexported state is kept.
func (s *SessionState) Save() ([]byte, error) { return []byte{byte(s.total)}, nil }
func (s *SessionState) Load(b []byte) error {
	if len(b) > 0 {
		s.total = int64(b[0])
	}
	return nil
}

// SpendIn is the parameter list for the spend method.
type SpendIn struct{ Amount int64 }

var Session = golem.DefineAgent[SessionID, SessionState](
	golem.Spec{
		Name:        "SessionAgent",
		Description: "A session that snapshots its running total every few invocations",
		Mode:        golem.Durable,
		// Snapshot the state every 5 invocations instead of the default cadence.
		Snapshot: golem.SnapshotEveryN(5),
	},
	func(id SessionID) *SessionState { return &SessionState{} },
)

var (
	Spend = golem.DefineMethod[SessionID, SpendIn, int64](
		"spend",
		golem.Desc("Add to the running total"),
	)
	Total = golem.DefineMethod[SessionID, golem.Unit, int64](
		"total",
		golem.Desc("Return the running total"),
	)
)

func init() {
	golem.Implement(Session, Spend, func(ctx *golem.Context[SessionState], in SpendIn) int64 {
		ctx.State.total += in.Amount
		return ctx.State.total
	})

	golem.Implement(Session, Total, func(ctx *golem.Context[SessionState], _ golem.Unit) int64 {
		return ctx.State.total
	})
}
