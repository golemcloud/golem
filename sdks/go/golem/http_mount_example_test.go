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

package golem_test

import (
	"fmt"

	"github.com/golemcloud/golem/sdks/go/golem"
)

func ExampleAgentIDPath() {
	// Set Spec.HTTP to this mount. Constructor inputs come from the create
	// request, not from the URL of a subsequent method or file request.
	mount := golem.Mount{
		Path:        "/chats/{id}",
		AgentID:     golem.AgentIDPath("id"),
		CreateRoute: "/chats",
		ExposeFiles: []golem.FileMapping{{Route: "/files", Path: "/data"}},
	}
	fmt.Println(mount.Path, mount.CreateRoute)
	// Output: /chats/{id} /chats
}

func ExampleAgentIDQuery() {
	mount := golem.Mount{
		Path:        "/chats",
		AgentID:     golem.AgentIDQuery("id"),
		CreateRoute: "/chats/create",
	}
	fmt.Println(mount.Path, mount.CreateRoute)
	// Output: /chats /chats/create
}
