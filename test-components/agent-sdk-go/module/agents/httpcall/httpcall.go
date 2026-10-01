// Package httpcall is the DEFINITION of the durable outbound-HTTP agent used
// by the replay tests. The behaviour lives in httpcall/impl.
package httpcall

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type CallbackIn struct{ Payload string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "HttpAgent", Description: "Durable outbound HTTP for replay tests", Mode: golem.Durable,
})

var Callback = Agent.Method[CallbackIn, string]("callback", golem.Desc("GET the PORT callback endpoint with the payload and return its body"))

// RetryCallback calls a flaky endpoint under a status-code retry policy.
var RetryCallback = Agent.Method[CallbackIn, string]("retry-callback", golem.Desc("GET the flaky endpoint under a retry policy that retries on 500"))

// AtomicTimedCallback reads the clock (wall + monotonic) inside the atomic
// region right before the HTTP call — a user-level stand-in for the runtime's
// sampled clock read that coincided with the pre-send hang.
var AtomicTimedCallback = Agent.Method[CallbackIn, string]("atomic-timed-callback", golem.Desc("time.Now() then GET, inside an atomic region"))

// AtomicCallback makes the same call inside golem.Atomically — the minimal case
// for "does an outbound HTTP call settle before an atomic region closes?".
var AtomicCallback = Agent.Method[CallbackIn, string]("atomic-callback", golem.Desc("GET the callback endpoint inside an atomic region"))
