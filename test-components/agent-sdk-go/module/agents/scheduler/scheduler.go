// Package scheduler is the DEFINITION of the agent exercising scheduled
// invocations (MethodDef.Schedule + ScheduledInvocation.Cancel). Behaviour lives
// in scheduler/impl.
package scheduler

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type ScheduleIn struct {
	// Target names the counter agent instance to bump.
	Target string
	// DelayMillis is how far in the future to schedule the bump.
	DelayMillis int64
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "SchedulerAgent", Description: "Exercises scheduled invocations", Mode: golem.Durable,
})

var (
	// Bump schedules a counter increment and lets it run.
	Bump = golem.DefineMethod[Id, ScheduleIn, golem.Unit]("bump",
		golem.Desc("Schedule an increment on the target counter"))
	// BumpCancelled schedules the same increment and immediately cancels it, so
	// the counter must never move.
	BumpCancelled = golem.DefineMethod[Id, ScheduleIn, golem.Unit]("bump-cancelled",
		golem.Desc("Schedule an increment on the target counter, then cancel it"))
)
