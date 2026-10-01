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
	"crypto/tls"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
)

// serveHTTP runs a net/http handler on a router request.
//
// The handler runs as the producer of the response body, on its own goroutine,
// because a net/http handler writes its response rather than returning it: the
// status and headers are handed back as soon as the handler commits them, and
// the invocation returns the response while the handler goes on writing the
// body.
func serveHTTP(ctx context.Context, h http.Handler, req HTTPRequest) HTTPResponse {
	r, err := toHTTPRequest(ctx, req)
	if err != nil {
		_ = req.Body.Close()
		return HTTPResponse{Status: http.StatusBadRequest, Headers: []HTTPHeader{}, Body: StreamOf[[]byte]()}
	}

	heads := make(chan responseHead, 1)
	body := ProduceStream(func(sw *AgentStreamWriter[[]byte]) (err error) {
		w := &responseWriter{header: http.Header{}, sw: sw, heads: heads, head: r.Method == http.MethodHead}
		defer func() { _ = r.Body.Close() }()
		defer func() {
			if p := recover(); p != nil {
				if !w.committed {
					heads <- responseHead{panicked: true, panicValue: p}
					return
				}
				// The head is already on its way, and a stream has no failure end
				// state: the body ends where the handler stopped.
				err = fmt.Errorf("golem: the HTTP handler panicked after sending the response head: %v", p)
				return
			}
			if !w.committed {
				w.commit(http.StatusOK)
			}
		}()
		h.ServeHTTP(w, r)
		return nil
	})

	head := <-heads
	if head.panicked {
		_ = body.Close()
		panic(head.panicValue)
	}
	return HTTPResponse{Status: head.status, Headers: head.headers, Body: body}
}

// toHTTPRequest builds the request a net/http server would have: the target
// parsed as a request URI, the authority as Host, and the body streamed.
func toHTTPRequest(ctx context.Context, req HTTPRequest) (*http.Request, error) {
	target := req.Path
	if q, ok := req.Query.Get(); ok {
		target += "?" + q
	}
	u, err := url.ParseRequestURI(target)
	if err != nil {
		return nil, err
	}
	header := http.Header{}
	for _, f := range req.Headers {
		header.Add(f.Name, string(f.Value))
	}
	// As in net/http, the authority is the request's Host, not a header.
	header.Del("Host")
	r := &http.Request{
		Method:        req.Method,
		URL:           u,
		Proto:         "HTTP/1.1",
		ProtoMajor:    1,
		ProtoMinor:    1,
		Header:        header,
		Host:          req.Authority,
		RequestURI:    target,
		Body:          &streamBody{s: req.Body},
		ContentLength: declaredLength(header),
	}
	if strings.EqualFold(req.Scheme, "https") {
		// net/http marks a request that arrived over TLS by a non-nil TLS
		// field; the host terminated it, so there is no state to report.
		r.TLS = &tls.ConnectionState{}
	}
	return r.WithContext(ctx), nil
}

// declaredLength reads a declared body length, or -1 when it is unknown.
func declaredLength(h http.Header) int64 {
	if v := h.Get("Content-Length"); v != "" {
		if n, err := strconv.ParseInt(v, 10, 64); err == nil && n >= 0 {
			return n
		}
	}
	return -1
}

// errBodyClosed is what reading a closed request body reports, as net/http's
// own body does.
var errBodyClosed = errors.New("http: invalid Read on closed Body")

// streamBody reads a request body stream as an io.ReadCloser.
type streamBody struct {
	s   AgentStream[[]byte]
	buf []byte
	err error
}

func (b *streamBody) Read(p []byte) (int, error) {
	for len(b.buf) == 0 {
		if b.err != nil {
			return 0, b.err
		}
		chunk, ok, err := b.s.Next()
		switch {
		case err != nil:
			// A failed or cancelled upload is never a complete one.
			b.err = err
		case !ok:
			b.err = io.EOF
		default:
			b.buf = chunk
		}
	}
	n := copy(p, b.buf)
	b.buf = b.buf[n:]
	return n, nil
}

func (b *streamBody) Close() error {
	if b.err == nil || b.err == io.EOF {
		b.err = errBodyClosed
	}
	b.buf = nil
	return b.s.Close()
}

// responseHead is what the handler commits before its body: the status and
// headers, or the panic that stopped it first.
type responseHead struct {
	status     uint16
	headers    []HTTPHeader
	panicked   bool
	panicValue any
}

// responseWriter is the http.ResponseWriter a router handler writes to. It
// follows net/http's rules: the head is committed by the first Write,
// WriteHeader or Flush, header changes after that are ignored, a missing
// Content-Type is sniffed from the first bytes, and a response that may not
// carry a body refuses one.
type responseWriter struct {
	header    http.Header
	sw        *AgentStreamWriter[[]byte]
	heads     chan<- responseHead
	head      bool // the request is a HEAD: accept body bytes but send none
	status    int
	committed bool
}

func (w *responseWriter) Header() http.Header { return w.header }

func (w *responseWriter) WriteHeader(code int) {
	if code < 100 || code > 999 {
		panic(fmt.Sprintf("invalid WriteHeader code %v", code))
	}
	// Interim responses belong to the host.
	if w.committed || code < 200 {
		return
	}
	w.commit(code)
}

func (w *responseWriter) Write(p []byte) (int, error) {
	if !w.committed {
		if _, typed := w.header["Content-Type"]; !typed && len(p) > 0 && bodyAllowed(http.StatusOK) {
			w.header.Set("Content-Type", http.DetectContentType(p))
		}
		w.commit(http.StatusOK)
	}
	if !bodyAllowed(w.status) {
		return 0, http.ErrBodyNotAllowed
	}
	if w.head || len(p) == 0 {
		return len(p), nil
	}
	// The caller may reuse p as soon as Write returns, and the chunk is still
	// in flight then.
	if err := w.sw.Write(append([]byte(nil), p...)); err != nil {
		return 0, err
	}
	return len(p), nil
}

// Flush commits the head; the body is never held back, so there is nothing
// else to flush.
func (w *responseWriter) Flush() {
	if !w.committed {
		w.commit(http.StatusOK)
	}
}

func (w *responseWriter) commit(status int) {
	w.committed = true
	w.status = status
	w.heads <- responseHead{status: uint16(status), headers: headerFields(w.header)}
}

// bodyAllowed reports whether a status may carry a body.
func bodyAllowed(status int) bool {
	return status != http.StatusNoContent && status != http.StatusNotModified && status >= 200
}

// headerFields flattens a header into fields: lowercase names, sorted, with the
// values of a repeated name kept separate and in order.
func headerFields(h http.Header) []HTTPHeader {
	names := make([]string, 0, len(h))
	for name := range h {
		names = append(names, name)
	}
	sort.Strings(names)
	out := make([]HTTPHeader, 0, len(names))
	for _, name := range names {
		lower := strings.ToLower(name)
		for _, v := range h[name] {
			out = append(out, HTTPHeader{Name: lower, Value: []byte(v)})
		}
	}
	return out
}
