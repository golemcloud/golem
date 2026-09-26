// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

package golem

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"reflect"
	"strings"
	"testing"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
)

func discoverOne(t *testing.T, d *definitions, name string) (common.AgentType, []definitionError) {
	t.Helper()
	types, errs := d.discover()
	for _, at := range types {
		if at.TypeName == name {
			return at, errs
		}
	}
	t.Fatalf("%s was not discovered; errors: %v", name, errs)
	return common.AgentType{}, nil
}

func echoRaw(_ context.Context, req HTTPRequest) HTTPResponse {
	return HTTPResponse{Status: 200, Body: req.Body}
}

func TestARouterPublishesTheRouterKind(t *testing.T) {
	d := newDefinitions()
	site := defineRouterInto[NoConfig](d, RouterSpec{
		Name:        "Website",
		Mount:       "/web/v1",
		Auth:        true,
		CORS:        []string{"*"},
		StaticFiles: []FileMapping{{Route: "/", Path: "/site/index.html"}},
	})
	site.HandleRaw(echoRaw)
	site.OpenAPI(func(context.Context) string { return "{}" })

	at, errs := discoverOne(t, d, "Website")
	if len(errs) > 0 {
		t.Fatalf("unexpected errors: %v", errs)
	}
	if at.Kind != common.AgentTypeKindHttpRouter || at.Mode != common.AgentModeEphemeral {
		t.Fatalf("kind %d, mode %d", at.Kind, at.Mode)
	}
	if at.Snapshotting.Tag() != common.SnapshottingDisabled {
		t.Fatalf("a router must not snapshot")
	}
	if n := len(at.Constructor.InputSchema.Parameters()); n != 0 {
		t.Fatalf("a router has no constructor parameters, got %d", n)
	}
	mount := at.HttpMount.Some()
	if got := renderSegs(mount.PathPrefix); got != "/web/v1" {
		t.Fatalf("mount = %s", got)
	}
	if mount.OpenapiProviderMethod.Some() != routerOpenAPIMethod || len(mount.StaticBindings) != 1 {
		t.Fatalf("mount = %+v", mount)
	}

	methods := map[string]common.AgentMethod{}
	for _, m := range at.Methods {
		methods[m.Name] = m
	}
	handle := methods[routerHandleMethod]
	if len(handle.HttpEndpoint) != 1 || handle.HttpEndpoint[0].HttpMethod.Tag() != common.HttpMethodAny ||
		len(handle.HttpEndpoint[0].PathSuffix) != 0 || handle.HttpEndpoint[0].AuthDetails.IsSome() {
		t.Fatalf("handler endpoint = %+v", handle.HttpEndpoint)
	}
	params := handle.InputSchema.Parameters()
	if len(params) != 1 || params[0].Name != "request" {
		t.Fatalf("the handler takes exactly one parameter named request, got %+v", params)
	}
	if provider := methods[routerOpenAPIMethod]; len(provider.HttpEndpoint) != 0 || len(provider.InputSchema.Parameters()) != 0 {
		t.Fatalf("the OpenAPI provider takes nothing and has no route: %+v", provider)
	}
}

// The host matches the handler's types structurally against its own request
// and response schemas, so the field names and their order are the contract.
func TestTheEnvelopeTypesHaveTheHostsFieldNames(t *testing.T) {
	d := newDefinitions()
	names := func(v any) string {
		var out []string
		for _, f := range d.structFields(reflect.TypeOf(v)) {
			out = append(out, f.name)
		}
		return strings.Join(out, ",")
	}
	if got := names(HTTPRequest{}); got != "method,scheme,authority,path,query,headers,body" {
		t.Fatalf("request fields = %s", got)
	}
	if got := names(HTTPResponse{}); got != "status,headers,body" {
		t.Fatalf("response fields = %s", got)
	}
	if got := names(HTTPHeader{}); got != "name,value" {
		t.Fatalf("header fields = %s", got)
	}
}

func TestAStaticOnlyRouterNeedsNoHandler(t *testing.T) {
	d := newDefinitions()
	defineRouterInto[NoConfig](d, RouterSpec{Name: "Static", Mount: "/", StaticFiles: []FileMapping{{Route: "/*", Path: "/$1"}}})
	if _, errs := discoverOne(t, d, "Static"); len(errs) > 0 {
		t.Fatalf("unexpected errors: %v", errs)
	}
}

func TestRouterMisuseIsADefinitionError(t *testing.T) {
	cases := []struct {
		name  string
		setup func(d *definitions)
		want  string
	}{
		{"captured mount", func(d *definitions) {
			defineRouterInto[NoConfig](d, RouterSpec{Name: "R", Mount: "/users/{id}"}).HandleRaw(echoRaw)
		}, "literal segments"},
		{"two handlers", func(d *definitions) {
			r := defineRouterInto[NoConfig](d, RouterSpec{Name: "R", Mount: "/r"})
			r.HandleRaw(echoRaw)
			r.Handle(http.NotFoundHandler())
		}, "already has a handler"},
		{"two providers", func(d *definitions) {
			r := defineRouterInto[NoConfig](d, RouterSpec{Name: "R", Mount: "/r"})
			r.OpenAPI(func(context.Context) string { return "" })
			r.OpenAPI(func(context.Context) string { return "" })
		}, "already has an OpenAPI provider"},
		{"bad static file", func(d *definitions) {
			defineRouterInto[NoConfig](d, RouterSpec{Name: "R", Mount: "/r", StaticFiles: []FileMapping{{Route: "/a/*", Path: "/b"}}})
		}, "target-placeholder"},
		{"nil handler", func(d *definitions) {
			defineRouterInto[NoConfig](d, RouterSpec{Name: "R", Mount: "/r"}).Handle(nil)
		}, "non-nil http.Handler"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			d := newDefinitions()
			c.setup(d)
			_, errs := d.discover()
			if !anyErrContains(errs, c.want) {
				t.Fatalf("want an error mentioning %q, got %v", c.want, errs)
			}
		})
	}
}

// Config reads the host, which a native test cannot link, so the scope check it
// makes first is tested on its own.
func TestConfigNeedsTheRoutersOwnRequest(t *testing.T) {
	r := defineRouterInto[NoConfig](newDefinitions(), RouterSpec{Name: "R", Mount: "/r"})
	checkRouterScope(r.scope(), "R")
	for _, ctx := range []context.Context{context.Background(), (&HTTPRouter[NoConfig]{name: "Other"}).scope()} {
		func() {
			defer func() {
				if p := recover(); p == nil || !strings.Contains(fmt.Sprint(p), "outside one of its requests") {
					t.Fatalf("got %v", p)
				}
			}()
			checkRouterScope(ctx, "R")
		}()
	}
}

// --- the net/http adapter ---

func request(method, path string, query Option[string], headers []HTTPHeader, chunks ...string) HTTPRequest {
	items := make([][]byte, len(chunks))
	for i, c := range chunks {
		items[i] = []byte(c)
	}
	return HTTPRequest{
		Method: method, Scheme: "https", Authority: "shop.example:8443", Path: path, Query: query,
		Headers: headers, Body: StreamOf(items...),
	}
}

func bodyOf(t *testing.T, resp HTTPResponse) (string, error) {
	t.Helper()
	chunks, err := resp.Body.Collect()
	var b strings.Builder
	for _, c := range chunks {
		b.Write(c)
	}
	return b.String(), err
}

func TestTheAdapterEchoesAStreamedBody(t *testing.T) {
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", r.Header.Get("Content-Type"))
		_, _ = io.Copy(w, r.Body)
	})
	req := request("POST", "/echo", None[string](),
		[]HTTPHeader{{Name: "content-type", Value: []byte("application/octet-stream")}}, "hel", "", "lo")
	resp := serveHTTP(context.Background(), h, req)
	if resp.Status != 200 {
		t.Fatalf("status %d", resp.Status)
	}
	if got := fmt.Sprint(resp.Headers); !strings.Contains(got, "content-type") {
		t.Fatalf("headers %v", resp.Headers)
	}
	if body, err := bodyOf(t, resp); err != nil || body != "hello" {
		t.Fatalf("body %q, %v", body, err)
	}
}

// The handler sees the request the client sent, the way a net/http server
// presents it.
func TestTheAdapterPresentsTheFullRequest(t *testing.T) {
	var seen *http.Request
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { seen = r })
	resp := serveHTTP(context.Background(), h,
		request("GET", "/web/a%2Fb/c", Some("x=1&x=2&y=a+b"), []HTTPHeader{{Name: "x-tenant", Value: []byte("acme")}}))
	_, _ = bodyOf(t, resp)
	switch {
	case seen.URL.Path != "/web/a/b/c" || seen.URL.RawPath != "/web/a%2Fb/c" || seen.URL.EscapedPath() != "/web/a%2Fb/c":
		t.Fatalf("path %q raw %q", seen.URL.Path, seen.URL.RawPath)
	case seen.URL.RawQuery != "x=1&x=2&y=a+b" || len(seen.URL.Query()["x"]) != 2:
		t.Fatalf("query %q", seen.URL.RawQuery)
	case seen.RequestURI != "/web/a%2Fb/c?x=1&x=2&y=a+b":
		t.Fatalf("request URI %q", seen.RequestURI)
	case seen.Host != "shop.example:8443" || seen.TLS == nil || seen.Header.Get("X-Tenant") != "acme":
		t.Fatalf("host %q, tls %v, header %v", seen.Host, seen.TLS, seen.Header)
	}
}

func TestRepeatedResponseHeadersStaySeparate(t *testing.T) {
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Add("Set-Cookie", "first=one")
		w.Header().Add("Set-Cookie", "second=two")
		w.WriteHeader(http.StatusCreated)
	})
	resp := serveHTTP(context.Background(), h, request("POST", "/c", None[string](), nil))
	_, _ = bodyOf(t, resp)
	if resp.Status != 201 {
		t.Fatalf("status %d", resp.Status)
	}
	var cookies []string
	for _, f := range resp.Headers {
		if f.Name == "set-cookie" {
			cookies = append(cookies, string(f.Value))
		}
	}
	if strings.Join(cookies, "|") != "first=one|second=two" {
		t.Fatalf("headers %v", resp.Headers)
	}
}

// The head goes out while the handler is still writing: here the handler
// blocks on a channel only the test's read of the body releases.
func TestTheHeadIsSentBeforeTheBodyEnds(t *testing.T) {
	release := make(chan struct{})
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, "first ")
		w.(http.Flusher).Flush()
		<-release
		_, _ = io.WriteString(w, "second")
	})
	resp := serveHTTP(context.Background(), h, request("GET", "/s", None[string](), nil))
	if resp.Status != 200 {
		t.Fatalf("status %d", resp.Status)
	}
	close(release)
	if body, err := bodyOf(t, resp); err != nil || body != "first second" {
		t.Fatalf("body %q, %v", body, err)
	}
}

func TestAPanicBeforeTheHeadFailsTheRequest(t *testing.T) {
	h := http.HandlerFunc(func(http.ResponseWriter, *http.Request) { panic("boom") })
	defer func() {
		if p := recover(); fmt.Sprint(p) != "boom" {
			t.Fatalf("got %v", p)
		}
	}()
	serveHTTP(context.Background(), h, request("GET", "/p", None[string](), nil))
}

// A stream has no failure end state, so a panic once the head is out can only
// end the body where the handler stopped.
func TestAPanicAfterTheHeadEndsTheBody(t *testing.T) {
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, "partial")
		panic("boom")
	})
	resp := serveHTTP(context.Background(), h, request("GET", "/p", None[string](), nil))
	if body, _ := bodyOf(t, resp); body != "partial" {
		t.Fatalf("body %q", body)
	}
}

func TestResponsesWithoutABodyRefuseOne(t *testing.T) {
	var werr error
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusNoContent)
		_, werr = w.Write([]byte("x"))
	})
	resp := serveHTTP(context.Background(), h, request("DELETE", "/d", None[string](), nil))
	if body, _ := bodyOf(t, resp); body != "" || werr != http.ErrBodyNotAllowed {
		t.Fatalf("body %q, write error %v", body, werr)
	}

	h = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = io.WriteString(w, "hidden") })
	resp = serveHTTP(context.Background(), h, request("HEAD", "/h", None[string](), nil))
	if body, _ := bodyOf(t, resp); body != "" {
		t.Fatalf("a HEAD response carried %q", body)
	}
}

func TestAnUntypedResponseIsSniffed(t *testing.T) {
	h := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { _, _ = io.WriteString(w, "<html></html>") })
	resp := serveHTTP(context.Background(), h, request("GET", "/", None[string](), nil))
	_, _ = bodyOf(t, resp)
	for _, f := range resp.Headers {
		if f.Name == "content-type" && strings.HasPrefix(string(f.Value), "text/html") {
			return
		}
	}
	t.Fatalf("headers %v", resp.Headers)
}
