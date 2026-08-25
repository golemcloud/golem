// Package impl is the IMPLEMENTATION of the blobstore agent. Each handler is a
// thin call into the SDK's blobstore wrapper, so the executor tests exercise the
// wrapper end to end.
package impl

import (
	"sort"

	"agent-sdk-go/agents/blobs"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/blobstore"
)

type state struct{}

var agent = golem.Implement(blobs.Agent, func(blobs.Id) *state { return &state{} })

func init() {
	golem.Handle(agent, blobs.Write, func(_ *golem.Context[state], in blobs.WriteIn) golem.Unit {
		c := golem.Must(blobstore.GetOrCreateContainer(in.Container))
		golem.Must0(c.WriteData(in.Object, []byte(in.Data)))
		return golem.Unit{}
	})
	golem.Handle(agent, blobs.Read, func(_ *golem.Context[state], in blobs.ObjectIn) string {
		c := golem.Must(blobstore.GetOrCreateContainer(in.Container))
		data, found := golem.Must2(c.GetData(in.Object))
		if !found {
			return ""
		}
		return string(data)
	})
	golem.Handle(agent, blobs.Size, func(_ *golem.Context[state], in blobs.ObjectIn) int64 {
		c := golem.Must(blobstore.GetOrCreateContainer(in.Container))
		info := golem.Must(c.ObjectInfo(in.Object))
		return int64(info.Size)
	})
	golem.Handle(agent, blobs.Delete, func(_ *golem.Context[state], in blobs.ObjectIn) golem.Unit {
		c := golem.Must(blobstore.GetOrCreateContainer(in.Container))
		golem.Must0(c.Delete(in.Object))
		return golem.Unit{}
	})
	golem.Handle(agent, blobs.List, func(_ *golem.Context[state], in blobs.ContainerIn) []string {
		c := golem.Must(blobstore.GetOrCreateContainer(in.Container))
		names := golem.Must(c.ListObjects())
		sort.Strings(names) // deterministic order for the test assertion
		return names
	})
}
