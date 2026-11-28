// A reflected Trigger or Schedule refuses a method that takes or returns a
// stream before any host call, as the typed MethodDef.Trigger and Schedule do.

package reflection

import (
	"testing"
	"time"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type streamerID struct{ Name string }

type streamer struct{}

var (
	streamerAgent = golem.DefineAgent[streamerID](golem.Spec{
		Name: "Streamer", Mode: golem.Durable,
	})
	streamerWatch = streamerAgent.Method[golem.Unit, golem.AgentStream[string]]("watch")
	_             = streamerAgent.Implement(func(streamerID) *streamer { return &streamer{} }).
			Handle(streamerWatch, func(*golem.Context[streamer], golem.Unit) golem.AgentStream[string] {
			return golem.StreamOf("a")
		})
)

func TestReflectedTriggerAndScheduleRefuseAStreamOutput(t *testing.T) {
	r := snapshotNamed(t, "Streamer")
	for _, form := range []string{"trigger", "schedule"} {
		rpc := &fakeRPC{}
		client := &AgentClient{agentType: r, id: knownAgentID(greeterID), rpc: rpc}
		var err error
		if form == "trigger" {
			_, err = client.Trigger("watch", map[string]any{})
		} else {
			_, err = client.Schedule(time.Now(), "watch", map[string]any{})
		}
		if rpc.gotForm != "" {
			t.Errorf("%s of a stream-returning method reached the host (%q)", form, rpc.gotForm)
		}
		if err == nil {
			t.Errorf("%s of a stream-returning method succeeded", form)
		}
	}
}
