// Package impl is the IMPLEMENTATION of the keyvalue agent. Each handler is a
// thin call into the SDK's keyvalue wrapper, so the executor tests exercise the
// wrapper end to end (including its durability: a key written in one invocation
// is still readable after a crash, replayed from the oplog).
package impl

import (
	"sort"

	"agent-sdk-go/agents/kvstore"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/keyvalue"
)

type state struct{}

var agent = golem.Implement(kvstore.Agent, func(kvstore.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, kvstore.Set, func(_ *golem.Context[state], in kvstore.SetIn) golem.Unit {
		b := golem.Must(keyvalue.OpenBucket(in.Bucket))
		golem.Must0(b.Set(in.Key, []byte(in.Value)))
		return golem.Unit{}
	})
	golem.Handle(agent, kvstore.Get, func(_ *golem.Context[state], in kvstore.GetIn) string {
		b := golem.Must(keyvalue.OpenBucket(in.Bucket))
		value, found := golem.Must2(b.Get(in.Key))
		if !found {
			return ""
		}
		return string(value)
	})
	golem.Handle(agent, kvstore.Exists, func(_ *golem.Context[state], in kvstore.GetIn) bool {
		b := golem.Must(keyvalue.OpenBucket(in.Bucket))
		return golem.Must(b.Exists(in.Key))
	})
	golem.Handle(agent, kvstore.Delete, func(_ *golem.Context[state], in kvstore.GetIn) golem.Unit {
		b := golem.Must(keyvalue.OpenBucket(in.Bucket))
		golem.Must0(b.Delete(in.Key))
		return golem.Unit{}
	})
	golem.Handle(agent, kvstore.Keys, func(_ *golem.Context[state], in kvstore.GetIn) []string {
		b := golem.Must(keyvalue.OpenBucket(in.Bucket))
		keys := golem.Must(b.Keys())
		sort.Strings(keys) // deterministic order for the test assertion
		return keys
	})
}
