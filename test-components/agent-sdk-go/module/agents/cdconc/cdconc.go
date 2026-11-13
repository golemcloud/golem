// Package cdconc is the DEFINITION of an agent that runs custom durable
// operations (durability.Run) on several goroutines of one invocation, to check
// which custom scope each host call is attributed to. Behaviour lives in
// cdconc/impl.
package cdconc

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "CdConcAgent", Description: "Custom durable operations on concurrent goroutines", Mode: golem.Durable,
})

var (
	Siblings            = Agent.Method[golem.Unit, string]("siblings", golem.Description("Two sibling operations; the second finishes first"))
	RootDuringBody      = Agent.Method[golem.Unit, string]("root-during-body", golem.Description("SDK host calls of a root goroutine while a body waits"))
	StdlibDuringBody    = Agent.Method[golem.Unit, string]("stdlib-during-body", golem.Description("Standard library host calls of a root goroutine while a body waits"))
	SleepInBody         = Agent.Method[golem.Unit, string]("sleep-in-body", golem.Description("A body sleeps on a Go timer"))
	SleepWithSibling    = Agent.Method[golem.Unit, string]("sleep-with-sibling", golem.Description("A body sleeps on a Go timer while a root goroutine makes host calls"))
	Children            = Agent.Method[golem.Unit, string]("children", golem.Description("A body starts goroutines that make host calls and joins them"))
	Nested              = Agent.Method[golem.Unit, string]("nested", golem.Description("Two concurrent nested operations under one parent"))
	Partial             = Agent.Method[golem.Unit, string]("partial", golem.Description("One completed and one incomplete sibling, then a root call"))
	Escaped             = Agent.Method[golem.Unit, string]("escaped", golem.Description("A body starts a goroutine that waits for code after the body"))
	RootSleepDuringBody = Agent.Method[golem.Unit, string]("root-sleep-during-body", golem.Description("A root goroutine sleeps on a Go timer while a body makes host calls"))
	GcInBody            = Agent.Method[golem.Unit, string]("gc-in-body", golem.Description("A body allocates enough to run the GC, then a root goroutine makes host calls"))
	GcAfterBody         = Agent.Method[golem.Unit, string]("gc-after-body", golem.Description("A sequential body allocates, then the same goroutine allocates and makes host calls"))
	BodyClockRootTimer  = Agent.Method[golem.Unit, string]("body-clock-root-timer", golem.Description("A body reads the clock without blocking while a root goroutine sleeps on a Go timer"))
	RandAfterBody       = Agent.Method[golem.Unit, string]("rand-after-body", golem.Description("A sequential body uses maps, then the same goroutine iterates a map and draws from math/rand"))
	Remembered          = Agent.Method[golem.Unit, string]("remembered", golem.Description("The value the last rand scenario kept in memory; makes no host call"))
	BodyTimerFirst      = Agent.Method[golem.Unit, string]("body-timer-first", golem.Description("A body's Go timer is due before a root goroutine's"))
	SiblingTimers       = Agent.Method[golem.Unit, string]("sibling-timers", golem.Description("Two sibling bodies sleep on Go timers; the second is due first"))
	AfterFuncInBody     = Agent.Method[golem.Unit, string]("after-func-in-body", golem.Description("A body waits for a time.AfterFunc callback that makes a host call"))
	TimerOutlivesBody   = Agent.Method[golem.Unit, string]("timer-outlives-body", golem.Description("A body returns with Go timers still pending, then the root sleeps and makes a host call"))
	Echo                = Agent.Method[golem.Unit, string]("echo", golem.Description("A plain root host call"))
)
