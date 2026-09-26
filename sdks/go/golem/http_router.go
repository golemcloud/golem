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
	"net/http"
	"reflect"

	common "github.com/golemcloud/golem/sdks/go/golem/internal/wit/golem_agent_common"
	witTypes "go.bytecodealliance.org/pkg/wit/types"
)

// HTTP routers.
//
// A router handles raw HTTP requests under a mount: it receives the request as
// the client sent it and streams back a response. Any [net/http.Handler] can be
// the handler — a ServeMux, a third-party router, a whole existing service:
//
//	var Site = golem.DefineHTTPRouter(golem.RouterSpec{Name: "Website", Mount: "/web"})
//
//	func init() {
//	    mux := http.NewServeMux()
//	    mux.HandleFunc("POST /web/echo", func(w http.ResponseWriter, r *http.Request) {
//	        io.Copy(w, r.Body)
//	    })
//	    Site.Handle(mux)
//	}
//
// A router is an agent of its own kind: it has no constructor parameters, a
// fresh instance serves each request, and it takes no snapshots. It cannot be
// called like an ordinary agent; it can call ordinary agents.

// RouterSpec describes an HTTP router.
type RouterSpec struct {
	// Name is the router's agent type name. Required.
	Name string
	// Description documents the router.
	Description string
	// Mount is the public path the router serves, e.g. "/web". It is made of
	// literal segments only: a router has no identity to capture.
	Mount string
	// Auth requires authentication for every request.
	Auth bool
	// CORS is the list of allowed-origin patterns; empty means none.
	CORS []string
	// StaticFiles serves files of the component, under the mount, before the
	// handler is consulted: a GET or HEAD that matches a mapping is answered
	// from the file, and a missing file falls through to the handler.
	StaticFiles []FileMapping
}

// HTTPHeader is one header field as it travels: a lowercase name and its raw
// bytes. A repeated field is several HTTPHeaders, in order.
type HTTPHeader struct {
	Name  string
	Value []byte
}

// HTTPRequest is a request exactly as the client sent it.
type HTTPRequest struct {
	Method    string
	Scheme    string
	Authority string
	// Path is the full public path, including the mount, still percent-encoded.
	Path string
	// Query is the raw query without "?"; absent and empty are different.
	Query   Option[string]
	Headers []HTTPHeader
	// Body is the request body, in chunks as they arrive. Read it at most once.
	Body AgentStream[[]byte]
}

// HTTPResponse is a response to stream back. The host frames it: do not add
// transfer encoding, and a Content-Length, if set, must match the body.
type HTTPResponse struct {
	Status  uint16
	Headers []HTTPHeader
	Body    AgentStream[[]byte]
}

// HTTPRouter is a defined router, returned by [DefineHTTPRouter] and
// [DefineConfiguredHTTPRouter]. Cfg is its config type.
type HTTPRouter[Cfg any] struct {
	name string
	d    *definitions
}

// DefineHTTPRouter declares a router without config. Put it in a package-level
// var and register its handler with [HTTPRouter.Handle] or
// [HTTPRouter.HandleRaw].
func DefineHTTPRouter(spec RouterSpec) *HTTPRouter[NoConfig] {
	return defineRouterInto[NoConfig](defs, spec)
}

// DefineConfiguredHTTPRouter declares a router with config type Cfg, which a
// handler reads with [HTTPRouter.Config].
func DefineConfiguredHTTPRouter[Cfg any](spec RouterSpec) *HTTPRouter[Cfg] {
	return defineRouterInto[Cfg](defs, spec)
}

// routerEntry is what an agent entry carries when it is a router.
type routerEntry struct {
	staticFiles []FileMapping
	// openAPI is set once a provider is registered.
	openAPI bool
}

// The method names a router's two roles are published under. The host finds
// the roles from the metadata, not the names, so these only need to be stable.
const (
	routerHandleMethod  = "handle"
	routerOpenAPIMethod = "openapi"
)

// routerHandleIn is the handler method's input: the host requires exactly one
// parameter, named "request".
type routerHandleIn struct {
	Request HTTPRequest
}

// routerID and routerState fill the type parameters an agent method needs; a
// router has neither an identity nor state.
type (
	routerID    struct{}
	routerState struct{}
)

func defineRouterInto[Cfg any](d *definitions, spec RouterSpec) *HTTPRouter[Cfg] {
	r := &HTTPRouter[Cfg]{name: spec.Name, d: d}
	if spec.Name == "" {
		d.recordErr("", "", "DefineHTTPRouter requires a non-empty RouterSpec.Name")
		return r
	}
	if _, dup := d.agents[spec.Name]; dup {
		d.recordErr(spec.Name, "", "agent type already defined")
		r.d = nil
		return r
	}
	e := &agentEntry{
		name:     spec.Name,
		desc:     spec.Description,
		mode:     common.AgentModeEphemeral,
		mount:    &Mount{Path: spec.Mount, Auth: spec.Auth, CORS: spec.CORS},
		snapshot: SnapshotDisabled,
		idType:   reflect.TypeFor[routerID](),
		methods:  map[string]*methodEntry{},
		newState: func(reflect.Value, string) any { return &routerState{} },
		router:   &routerEntry{staticFiles: spec.StaticFiles},
	}
	d.agents[spec.Name] = e
	d.order = append(d.order, spec.Name)
	flattenConfigStruct(d, e, spec.Name, reflect.TypeFor[Cfg]())
	return r
}

func (r *HTTPRouter[Cfg]) entry() *agentEntry {
	if r == nil || r.d == nil {
		return nil
	}
	return r.d.agents[r.name]
}

// Handle serves the router's requests with an [net/http.Handler].
//
// The handler sees the request as the client sent it: r.URL.Path is the full
// public path, mount included — wrap it in [net/http.StripPrefix] to route
// relative to the mount — r.Host is the authority, and r.Body streams the
// request body. The response streams too: its status and headers are sent on
// the first Write, WriteHeader or Flush, or when the handler returns, and the
// handler may keep writing after that.
//
// A panic before the status is sent fails the request. After it, the body can
// only end where the handler stopped: set Content-Length when a client must be
// able to tell a truncated response from a complete one.
func (r *HTTPRouter[Cfg]) Handle(h http.Handler) Registered {
	if h == nil {
		if e := r.entry(); e != nil {
			r.d.recordErr(e.name, routerHandleMethod, "Handle requires a non-nil http.Handler")
		}
		return Registered{}
	}
	return r.HandleRaw(func(ctx context.Context, req HTTPRequest) HTTPResponse {
		return serveHTTP(ctx, h, req)
	})
}

// HandleRaw serves the router's requests from the exact envelope: byte-valued
// headers in the order they arrived, and the raw path and query. Use it when
// [net/http]'s view of a request loses something the handler needs.
func (r *HTTPRouter[Cfg]) HandleRaw(h func(ctx context.Context, req HTTPRequest) HTTPResponse) Registered {
	e := r.entry()
	if e == nil {
		return Registered{}
	}
	if h == nil {
		r.d.recordErr(e.name, routerHandleMethod, "HandleRaw requires a non-nil handler")
		return Registered{}
	}
	if _, dup := e.methods[routerHandleMethod]; dup {
		r.d.recordErr(e.name, routerHandleMethod, "the router already has a handler")
		return Registered{}
	}
	m := MethodDef[routerID, routerHandleIn, HTTPResponse]{name: routerHandleMethod, desc: "Handles an HTTP request"}
	bindMethodInto[routerID, routerState, routerHandleIn, HTTPResponse](r.d, e, m,
		func(_ *Context[routerState], in routerHandleIn) HTTPResponse {
			return h(r.scope(), in.Request)
		})
	return Registered{}
}

// OpenAPI registers a provider for the router's OpenAPI document: an OpenAPI
// 3.1.0 JSON object whose paths are relative to the mount. The host validates
// it, places it under the mount and merges it into the deployment's document.
func (r *HTTPRouter[Cfg]) OpenAPI(provide func(ctx context.Context) string) Registered {
	e := r.entry()
	if e == nil {
		return Registered{}
	}
	if provide == nil {
		r.d.recordErr(e.name, routerOpenAPIMethod, "OpenAPI requires a non-nil provider")
		return Registered{}
	}
	if e.router.openAPI {
		r.d.recordErr(e.name, routerOpenAPIMethod, "the router already has an OpenAPI provider")
		return Registered{}
	}
	e.router.openAPI = true
	m := MethodDef[routerID, Unit, string]{name: routerOpenAPIMethod, desc: "Returns the router's OpenAPI document"}
	bindMethodInto[routerID, routerState, Unit, string](r.d, e, m,
		func(_ *Context[routerState], _ Unit) string {
			return provide(r.scope())
		})
	return Registered{}
}

// routerScopeKey keys the router a context belongs to.
type routerScopeKey struct{}

func (r *HTTPRouter[Cfg]) scope() context.Context {
	return context.WithValue(context.Background(), routerScopeKey{}, r.name)
}

// Config returns the router's config. ctx must be the context of one of this
// router's requests — r.Context() in a [net/http.Handler], or the ctx passed
// to a raw handler or OpenAPI provider — since config exists only while the
// router serves a request.
//
// Like an agent's config it is read from the host once per worker; a [Secret]
// field re-reads the host on each Get.
func (r *HTTPRouter[Cfg]) Config(ctx context.Context) Cfg {
	checkRouterScope(ctx, r.name)
	return materializeAgentConfig[Cfg]()
}

// checkRouterScope panics unless ctx belongs to a request of the named router.
func checkRouterScope(ctx context.Context, router string) {
	if name, _ := ctx.Value(routerScopeKey{}).(string); name != router {
		panic(fmt.Errorf("golem: %s.Config called outside one of its requests", router))
	}
}

// buildRouterHTTP compiles a router's mount: literal segments only, its static
// files, the handler's catch-all binding and the OpenAPI provider's role.
func buildRouterHTTP(e *agentEntry, mp parsedPath, rec func(method, format string, args ...any)) (witTypes.Option[common.HttpMountDetails], map[string][]common.HttpEndpointDetails) {
	for _, s := range mp.segs {
		if s.kind != literalSeg {
			rec("", "router mount %q must be made of literal segments; a router has no identity to capture", e.mount.Path)
			break
		}
	}
	files, ferrs := compileFileMappings(e.router.staticFiles)
	for _, fe := range ferrs {
		rec("", "StaticFiles %s", fe)
	}
	provider := witTypes.None[string]()
	if e.router.openAPI {
		provider = witTypes.Some(routerOpenAPIMethod)
	}
	mount := common.HttpMountDetails{
		PathPrefix:    witSegments(mp.segs),
		AuthDetails:   witTypes.Some(common.AuthDetails{Required: e.mount.Auth}),
		PhantomAgent:  false,
		CorsOptions:   common.CorsOptions{AllowedPatterns: e.mount.CORS},
		WebhookSuffix: []common.PathSegment{},

		StaticBindings:        files,
		FilesystemBindings:    []common.FileMapping{},
		OpenapiProviderMethod: provider,
	}
	endpoints := map[string][]common.HttpEndpointDetails{}
	if _, ok := e.methods[routerHandleMethod]; ok {
		// Every request under the mount reaches the handler: any method, no
		// suffix, and no per-endpoint policy — auth and CORS are the mount's.
		endpoints[routerHandleMethod] = []common.HttpEndpointDetails{{
			HttpMethod:     common.MakeHttpMethodAny(),
			PathSuffix:     []common.PathSegment{},
			HeaderVars:     []common.HeaderVariable{},
			QueryVars:      []common.QueryVariable{},
			AuthDetails:    witTypes.None[common.AuthDetails](),
			CorsOptions:    common.CorsOptions{AllowedPatterns: []string{}},
			DurableStreams: witTypes.None[common.DurableStreamRouteOptions](),
		}}
	}
	return witTypes.Some(mount), endpoints
}
