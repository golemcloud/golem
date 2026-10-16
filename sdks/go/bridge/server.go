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

package bridge

import "strings"

// localWellKnownToken is the token the single-executable local server accepts.
// It is a fixed development credential, not a secret.
const localWellKnownToken = "5c832d93-ff85-4a8f-9803-513950fdfdb1"

// Server is where a client sends its calls: a base URL and the token to present
// with them. Build one with Local, Cloud or Custom rather than by hand, so a
// zero Server is always recognisably unconfigured.
type Server struct {
	url   string
	token string
}

// Local is the single-executable Golem server on its default port.
func Local() Server {
	return Server{url: "http://localhost:9881", token: localWellKnownToken}
}

// Cloud is the Golem Cloud release region.
func Cloud(token string) Server {
	return Server{url: "https://release.api.golem.cloud", token: token}
}

// Custom is any other worker-service deployment.
func Custom(url, token string) Server {
	return Server{url: strings.TrimSuffix(url, "/"), token: token}
}

// URL is the worker service's base URL, without a trailing slash.
func (s Server) URL() string { return s.url }

// Token is the bearer token sent with every request.
func (s Server) Token() string { return s.token }

// IsZero reports whether this is the zero Server, which names nothing.
func (s Server) IsZero() bool { return s.url == "" }
