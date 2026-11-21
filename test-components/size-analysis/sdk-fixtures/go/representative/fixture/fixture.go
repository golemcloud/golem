package fixture

import (
	"github.com/dustin/go-humanize"
	"github.com/golemcloud/golem/sdks/go/golem"
	stdinprobe "golem.local/bridge/stdin-probe-tool-guest-client"
)

const payload = "__SIZE_PAYLOAD__"

type ID struct{ Name string }
type HelperID struct{ Name string }
type Settings struct{ Label string }
type AddIn struct{ By int64 }
type SliceIn struct{ Start, Length uint32 }

var Counter = golem.DefineConfiguredAgent[ID, Settings](golem.Spec{
	Name: "CounterAgent",
	HTTP: &golem.Mount{Path: "/counters/{name}"},
})
var (
	Value         = Counter.Method[golem.Unit, int64]("value", golem.HTTP(golem.GET("/value")))
	Add           = Counter.Method[AddIn, int64]("add")
	Configured    = Counter.Method[golem.Unit, string]("configured")
	PayloadLength = Counter.Method[golem.Unit, uint32]("payloadLength")
	Slice         = Counter.Method[SliceIn, string]("slice")
	Dependency    = Counter.Method[SliceIn, string]("dependency")
	RPCAdd        = Counter.Method[AddIn, int64]("rpcAdd")
	Tool          = Counter.Method[golem.Unit, string]("tool")
)

var Helper = golem.DefineAgent[HelperID](golem.Spec{Name: "SizeHelper"})
var HelperValue = Helper.Method[golem.Unit, int64]("value")
var HelperAdd = Helper.Method[AddIn, int64]("add")

type state struct {
	name  string
	count int64
}

func init() {
	impl := Counter.ImplementConfigured(func(ctx *golem.InitContext[ID, state, Settings]) *state {
		return &state{name: ctx.ID().Name}
	})
	impl.Handle(Value, func(ctx *golem.Context[state], _ golem.Unit) int64 { return ctx.State.count })
	impl.Handle(Add, func(ctx *golem.Context[state], in AddIn) int64 {
		ctx.State.count += in.By
		return ctx.State.count
	})
	impl.Handle(Configured, func(ctx *golem.Context[state], _ golem.Unit) string { return ctx.Config(Counter).Label })
	impl.Handle(PayloadLength, func(_ *golem.Context[state], _ golem.Unit) uint32 { return uint32(len(payload)) })
	impl.Handle(Slice, func(_ *golem.Context[state], in SliceIn) string { return payload[in.Start : in.Start+in.Length] })
	impl.Handle(Dependency, func(_ *golem.Context[state], in SliceIn) string {
		return humanize.Bytes(uint64(len(payload[in.Start : in.Start+in.Length])))
	})
	impl.Handle(RPCAdd, func(ctx *golem.Context[state], in AddIn) int64 {
		return HelperAdd.MustCall(Helper.Upsert(HelperID{Name: ctx.State.name}), in)
	})
	impl.Handle(Tool, func(_ *golem.Context[state], _ golem.Unit) string {
		if _, err := stdinprobe.Ignore.Call(nil); err != nil {
			panic(err)
		}
		return "ignored"
	})
	helper := Helper.Implement(func(id HelperID) *state { return &state{name: id.Name} })
	helper.Handle(HelperValue, func(ctx *golem.Context[state], _ golem.Unit) int64 { return ctx.State.count })
	helper.Handle(HelperAdd, func(ctx *golem.Context[state], in AddIn) int64 {
		ctx.State.count += in.By
		return ctx.State.count
	})
}
