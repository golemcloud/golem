// Package kvstore is the DEFINITION of the agent exercising the SDK's keyvalue
// wrapper (golem/keyvalue). Behaviour lives in kvstore/impl.
package kvstore

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type SetIn struct {
	Bucket string
	Key    string
	Value  string
}

type GetIn struct {
	Bucket string
	Key    string
}

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "KvAgent", Description: "Exercises the Go SDK keyvalue wrapper", Mode: golem.Durable,
})

var (
	Set = golem.DefineMethod[Id, SetIn, golem.Unit]("set",
		golem.Desc("Store a string value under a key"))
	// Get returns the stored value, or "" when the key is absent.
	Get = golem.DefineMethod[Id, GetIn, string]("get",
		golem.Desc("Read a key; empty string when absent"))
	Exists = golem.DefineMethod[Id, GetIn, bool]("exists",
		golem.Desc("Report whether a key is present"))
	Delete = golem.DefineMethod[Id, GetIn, golem.Unit]("delete",
		golem.Desc("Delete a key"))
	// Keys returns the bucket's keys, sorted so the result is comparable.
	Keys = golem.DefineMethod[Id, GetIn, []string]("keys",
		golem.Desc("List the bucket's keys, sorted"))
)
