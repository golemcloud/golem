package index

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/golemcloud/golem/sdks/go/golem"
)

const marker = "revision-0"

type MultipartIndexID struct{ Name string }

type AppendIn struct{ Byte uint8 }

type saved struct {
	Revision uint32 `json:"revision"`
}

type state struct {
	revision uint32
	index    []byte
	restored bool
}

func (s *state) SaveParts() (golem.MultipartSnapshot, error) {
	return golem.NewMultipartSnapshot(saved{Revision: s.revision}, map[string]golem.SnapshotPart{
		"index": {Bytes: s.index, ContentType: "application/octet-stream"},
	})
}

func (s *state) LoadParts(snapshot golem.MultipartSnapshot) error {
	var st saved
	if err := snapshot.DecodeState(&st); err != nil {
		return err
	}
	index, err := snapshot.RequirePart("index", "application/octet-stream")
	if err != nil {
		return err
	}
	*s = state{revision: st.Revision, index: index, restored: true}
	return nil
}

var MultipartIndex = golem.DefineAgent[MultipartIndexID](golem.Spec{
	Name:     "MultipartIndex",
	Snapshot: golem.SnapshotEveryN(10),
})

var (
	appendByte = MultipartIndex.Method[AppendIn, golem.Unit]("append")
	inspect    = MultipartIndex.Method[golem.Unit, string]("inspect")
)

func init() {
	impl := MultipartIndex.Implement(func(MultipartIndexID) *state {
		index := make([]byte, 256)
		for i := range index {
			index[i] = byte(i)
		}
		return &state{index: index}
	})
	impl.Handle(appendByte, func(ctx *golem.Context[state], in AppendIn) golem.Unit {
		ctx.State.revision++
		ctx.State.index = append(ctx.State.index, in.Byte)
		return golem.Unit{}
	})
	impl.Handle(inspect, func(ctx *golem.Context[state], _ golem.Unit) string {
		bytes := make([]string, len(ctx.State.index))
		for i, b := range ctx.State.index {
			bytes[i] = strconv.Itoa(int(b))
		}
		return fmt.Sprintf("%s|%d|%t|%s", marker, ctx.State.revision, ctx.State.restored, strings.Join(bytes, ","))
	})
}
