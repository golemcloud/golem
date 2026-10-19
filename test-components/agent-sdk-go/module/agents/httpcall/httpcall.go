// Package httpcall is the DEFINITION of the durable outbound-HTTP agent used
// by the replay tests. The behaviour lives in httpcall/impl.
package httpcall

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallbackIn struct{ Payload string }

type RunParallelIn struct{ N uint16 }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "HttpAgent", Description: "Durable outbound HTTP for replay tests", Mode: golem.Durable,
})

var Callback = Agent.Method[CallbackIn, string]("callback", golem.Description("GET the PORT callback endpoint with the payload and return its body"))

// RetryCallback calls a flaky endpoint under a status-code retry policy.
var RetryCallback = Agent.Method[CallbackIn, string]("retry-callback", golem.Description("GET the flaky endpoint under a retry policy that retries on 500"))

// ScheduleCallback calls the flaky endpoint and retries it in the agent, with a
// retry.Schedule that sleeps between the attempts, while it answers 500.
var ScheduleCallback = Agent.Method[CallbackIn, string]("schedule-callback", golem.Description("GET the flaky endpoint, retrying in the agent with a schedule while it answers 500; return attempts:body"))

// ScheduleAttempts reports how many attempts the schedule runs made, which a
// replay rebuilds from the oplog.
var ScheduleAttempts = Agent.Method[golem.Unit, uint32]("schedule-attempts", golem.Description("The attempts of every schedule-callback so far"))

// TransactionCallback runs a fallible transaction whose two steps each GET the
// callback endpoint.
var TransactionCallback = Agent.Method[CallbackIn, string]("transaction-callback", golem.Description("GET the callback endpoint from two transaction steps and join the bodies"))

// ReadTruncated GETs /truncated, whose response ends before its Content-Length,
// and reports whether reading the body failed.
var ReadTruncated = Agent.Method[golem.Unit, string]("read-truncated", golem.Description("GET a truncated response and return the body read error, or the body"))

// PostFailingBody POSTs a body whose reader fails after the first chunk, and
// reports whether the request failed.
var PostFailingBody = Agent.Method[golem.Unit, string]("post-failing-body", golem.Description("POST a body whose reader fails midway and return the request error, or the response"))

var RunParallel = Agent.Method[RunParallelIn, []string]("run-parallel", golem.Description("POST n requests from n goroutines at once, each tagged with its X-Test id, and return the bodies in id order"))

type SpanContextIn struct {
	Key   string
	Value string
}

var SpanContext = Agent.Method[SpanContextIn, string]("span-context", golem.Description("Set an attribute on a custom span and read it back through the invocation context"))
