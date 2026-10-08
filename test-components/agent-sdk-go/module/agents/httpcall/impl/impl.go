// Package impl is the IMPLEMENTATION of the outbound-HTTP agent. The
// SDK routes net/http through the durable wasi:http transport, so the response is
// recorded in the oplog and served from it on replay after a restart rather than
// re-fetched (the exactly-once test asserts this via an external counter).
package impl

import (
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"agent-sdk-go/agents/httpcall"

	"github.com/golemcloud/golem/sdks/go/golem"
	"github.com/golemcloud/golem/sdks/go/golem/durability"
	"github.com/golemcloud/golem/sdks/go/golem/invocation"
	"github.com/golemcloud/golem/sdks/go/golem/retry"
)

type state struct{}

func fetch(payload string) string {
	url := "http://localhost:" + os.Getenv("PORT") + "/callback?payload=" + payload
	resp := golem.Must(http.Get(url))
	defer resp.Body.Close()
	return string(golem.Must(io.ReadAll(resp.Body)))
}

// post sends one tagged request; the server can hold and release each response
// individually by its X-Test id.
func post(id int) string {
	url := "http://localhost:" + os.Getenv("PORT") + "/post-example"
	req := golem.Must(http.NewRequest(http.MethodPost, url, strings.NewReader(fmt.Sprintf("body %d", id))))
	req.Header.Set("X-Test", fmt.Sprint(id))
	resp := golem.Must(http.DefaultClient.Do(req))
	defer resp.Body.Close()
	return string(golem.Must(io.ReadAll(resp.Body)))
}

var agent = httpcall.Agent.Implement(func(httpcall.Id) *state { return &state{} })

func init() {
	agent.Handle(httpcall.Callback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		return fetch(in.Payload)
	})
	agent.Handle(httpcall.RetryCallback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		// Retry the request while the endpoint answers 500; the host re-issues it
		// transparently, so the handler just sees the eventual success.
		pol := retry.Immediate().MaxRetries(10).OnlyWhen(retry.StatusCode.OneOf(500))
		defer retry.With(retry.Named("flaky-endpoint", pol).WithPriority(10))()
		return fetch(in.Payload)
	})
	agent.Handle(httpcall.AtomicTimedCallback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		var body string
		durability.Atomically(func() {
			started := time.Now()
			body = fetch(in.Payload)
			_ = time.Since(started)
		})
		return body
	})
	agent.Handle(httpcall.RunParallel, func(_ *golem.Context[state], in httpcall.RunParallelIn) []string {
		bodies := make([]string, in.N)
		var wg sync.WaitGroup
		for i := range int(in.N) {
			wg.Go(func() { bodies[i] = post(i) })
		}
		wg.Wait()
		return bodies
	})
	agent.Handle(httpcall.SpanContext, func(_ *golem.Context[state], in httpcall.SpanContextIn) string {
		span := invocation.StartSpan("go-span")
		span.SetAttribute(in.Key, in.Value)
		inside := invocation.CurrentContext()
		value, _ := inside.Attribute(in.Key, false)
		_, hasParent := inside.Parent()
		started := !span.StartedAt().IsZero()
		span.Finish()
		_, after := invocation.CurrentContext().Attribute(in.Key, false)
		return fmt.Sprintf("trace=%t value=%s parent=%t started=%t after=%t",
			inside.TraceID() != "", value, hasParent, started, after)
	})
	agent.Handle(httpcall.AtomicCallback, func(_ *golem.Context[state], in httpcall.CallbackIn) string {
		var body string
		durability.Atomically(func() { body = fetch(in.Payload) })
		return body
	})
}
