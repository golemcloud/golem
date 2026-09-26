// Package routers is the Go HTTP router fixture driven by go_http_router.rs.
package routers

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"os"

	"github.com/golemcloud/golem/sdks/go/golem"
)

// WebRouter serves a net/http mux and provides an OpenAPI fragment.
var WebRouter = golem.DefineHTTPRouter(golem.RouterSpec{Name: "WebRouter", Mount: "/web"})

// RawRouter answers from the exact envelope.
var RawRouter = golem.DefineHTTPRouter(golem.RouterSpec{Name: "RawRouter", Mount: "/raw"})

// StaticRouter only serves component files.
var StaticRouter = golem.DefineHTTPRouter(golem.RouterSpec{
	Name:        "StaticRouter",
	Mount:       "/static",
	StaticFiles: []golem.FileMapping{{Route: "/*", Path: "/assets/$1"}},
})

type GreetingConfig struct{ Greeting string }

// ConfiguredRouter reads its config from the request context.
var ConfiguredRouter = golem.DefineConfiguredHTTPRouter[GreetingConfig](golem.RouterSpec{
	Name:  "ConfiguredRouter",
	Mount: "/configured",
})

type FilesID struct{ Name string }

// Files is an ordinary agent exposing a file it rewrites.
var Files = golem.DefineAgent[FilesID](golem.Spec{
	Name: "Files",
	HTTP: &golem.Mount{
		Path:        "/files/{name}",
		ExposeFiles: []golem.FileMapping{{Route: "/value", Path: "/value.txt"}},
	},
})

var Update = Files.Method[golem.Unit, string]("update")

type filesState struct{}

func must(err error) {
	if err != nil {
		panic(err)
	}
}

func init() {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /web/early", func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusAccepted)
		_, _ = io.WriteString(w, "early")
	})
	mux.HandleFunc("GET /web/head", func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, "not sent for HEAD")
	})
	mux.HandleFunc("POST /web/echo", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Add("Set-Cookie", "a=first")
		w.Header().Add("Set-Cookie", "a=second")
		w.(http.Flusher).Flush()
		buf := make([]byte, 64)
		for {
			n, err := r.Body.Read(buf)
			if n > 0 {
				if _, werr := w.Write(buf[:n]); werr != nil {
					return
				}
			}
			if err != nil {
				return
			}
		}
	})
	WebRouter.Handle(mux)
	WebRouter.OpenAPI(func(context.Context) string {
		return `{"openapi":"3.1.0","info":{"title":"Router","version":"1"},` +
			`"paths":{"/echo":{"post":{"operationId":"echo","responses":{"200":{"description":"Streaming echo"}}}}}}`
	})

	RawRouter.HandleRaw(func(_ context.Context, req golem.HTTPRequest) golem.HTTPResponse {
		_ = req.Body.Close()
		var query any
		if q, ok := req.Query.Get(); ok {
			query = q
		}
		body, err := json.Marshal(map[string]any{"method": req.Method, "path": req.Path, "query": query})
		must(err)
		return golem.HTTPResponse{
			Status:  200,
			Headers: []golem.HTTPHeader{{Name: "content-type", Value: []byte("application/json")}},
			Body:    golem.StreamOf(body),
		}
	})

	ConfiguredRouter.Handle(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, ConfiguredRouter.Config(r.Context()).Greeting)
	}))

	files := Files.Implement(func(id FilesID) *filesState {
		must(os.WriteFile("/value.txt", []byte("initial:"+id.Name), 0o644))
		return &filesState{}
	})
	files.Handle(Update, func(*golem.Context[filesState], golem.Unit) string {
		must(os.WriteFile("/value.txt", []byte("updated"), 0o644))
		return "updated"
	})
}
