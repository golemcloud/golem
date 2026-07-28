package main

import "github.com/golemcloud/golem/sdks/go/golem"

// SessionAgent is ephemeral: it has no durable identity, so a client for it is
// obtained with golem.NewPhantom (see the shop's checkout). It tracks per-session
// events in memory for the life of the worker.
type SessionId struct{ Label string }
type SessionState struct{ events int64 }

var Session = golem.DefineAgent[SessionId, SessionState](
	golem.Spec{Name: "SessionAgent", Description: "An ephemeral per-request session", Mode: golem.Ephemeral},
	func(id SessionId) *SessionState { return &SessionState{} },
)

type TrackIn struct{ Event string }

var Track = golem.DefineMethod[SessionId, TrackIn, int64](
	"track",
	golem.Desc("Record an event, returning the running count"),
)

func init() {
	golem.Implement(Session, Track, func(ctx *golem.Context[SessionState], _ TrackIn) int64 {
		ctx.State.events++
		return ctx.State.events
	})
}
