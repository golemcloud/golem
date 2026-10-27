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

import (
	"fmt"
	"net/http"
	"sync"
	"time"
)

const (
	// DefaultReconnectWindow is the reconnect window of a configuration that
	// does not set one.
	DefaultReconnectWindow = 5 * time.Minute
	// NoReconnectLimit is a reconnect window without a limit: a streaming
	// call keeps reconnecting until its context ends.
	NoReconnectLimit time.Duration = -1
)

// Configuration is what a generated client needs to reach an agent: the server,
// and the application and environment the agent lives in.
type Configuration struct {
	Server  Server
	AppName string
	EnvName string
	// HTTPClient sends the requests. A nil client means http.DefaultClient,
	// which is fine for a script and wrong for a service that needs its own
	// timeouts and connection pool.
	HTTPClient *http.Client
	// ReconnectWindow is how long a streaming call keeps reconnecting while
	// the server accepts neither its start nor a resume. After the window the
	// call fails with an *UnreachableError. Zero means DefaultReconnectWindow;
	// NoReconnectLimit removes the limit.
	ReconnectWindow time.Duration
}

// reconnectWindow is the effective window; zero means no limit.
func (c Configuration) reconnectWindow() time.Duration {
	switch c.ReconnectWindow {
	case 0:
		return DefaultReconnectWindow
	case NoReconnectLimit:
		return 0
	}
	return c.ReconnectWindow
}

// client is the HTTP client to send with, never nil.
func (c Configuration) client() *http.Client {
	if c.HTTPClient != nil {
		return c.HTTPClient
	}
	return http.DefaultClient
}

func (c Configuration) validate() error {
	switch {
	case c.Server.IsZero():
		return fmt.Errorf("golem: no server configured; build one with bridge.Local, bridge.Cloud or bridge.Custom")
	case c.AppName == "":
		return fmt.Errorf("golem: no application name configured")
	case c.EnvName == "":
		return fmt.Errorf("golem: no environment name configured")
	case c.ReconnectWindow < 0 && c.ReconnectWindow != NoReconnectLimit:
		return fmt.Errorf("golem: the reconnect window must be positive, zero or bridge.NoReconnectLimit")
	}
	return nil
}

// The ambient configuration generated clients read when they are not handed one
// explicitly. It is replaceable rather than write-once so a test or a REPL can
// point the same process at a different server.
var ambient struct {
	sync.RWMutex
	configuration *Configuration
}

// Configure sets the configuration every generated client in this process uses
// by default. Calling it again repoints them.
func Configure(c Configuration) error {
	if err := c.validate(); err != nil {
		return err
	}
	ambient.Lock()
	defer ambient.Unlock()
	ambient.configuration = &c
	return nil
}

// Current returns the ambient configuration, or an error naming what to call if
// it was never set.
func Current() (Configuration, error) {
	ambient.RLock()
	defer ambient.RUnlock()
	if ambient.configuration == nil {
		return Configuration{}, fmt.Errorf(
			"golem: the bridge is not configured; call bridge.Configure first")
	}
	return *ambient.configuration, nil
}

// IsConfigured reports whether Configure has been called.
func IsConfigured() bool {
	ambient.RLock()
	defer ambient.RUnlock()
	return ambient.configuration != nil
}
