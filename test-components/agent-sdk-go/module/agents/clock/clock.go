// Package clock is the DEFINITION of the durable-clock test agent. It records
// wall-clock readings into durable state so a replay after an executor restart
// can be shown to reproduce the exact recorded time rather than reading the
// clock afresh. Behaviour lives in clock/impl.
package clock

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "ClockAgent", Description: "Durable wall-clock readings for replay tests", Mode: golem.Durable,
})

var (
	RecordTime = golem.DefineMethod[Id, golem.Unit, int64]("record-time",
		golem.Desc("Read time.Now() into durable state and return it as unix nanos"))
	FirstTime = golem.DefineMethod[Id, golem.Unit, int64]("first-time",
		golem.Desc("Return the first recorded reading as unix nanos"))
)
