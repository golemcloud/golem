// Package blobs is the DEFINITION of the agent exercising the SDK's blobstore
// wrapper (golem/blobstore). Behaviour lives in blobs/impl.
package blobs

import "github.com/golemcloud/golem/sdks/go/golem"

type Id struct{ Name string }

type WriteIn struct {
	Container string
	Object    string
	Data      string
}

type ObjectIn struct {
	Container string
	Object    string
}

type ContainerIn struct{ Container string }

var Agent = golem.DefineAgent[Id](golem.Spec{
	Name: "BlobAgent", Description: "Exercises the Go SDK blobstore wrapper", Mode: golem.Durable,
})

var (
	Write = golem.DefineMethod[Id, WriteIn, golem.Unit]("write",
		golem.Desc("Write an object into a container, creating the container if needed"))
	// Read returns the object's content, or "" when the object is absent.
	Read = golem.DefineMethod[Id, ObjectIn, string]("read",
		golem.Desc("Read an object; empty string when absent"))
	Size = golem.DefineMethod[Id, ObjectIn, int64]("size",
		golem.Desc("Return the object's size in bytes"))
	Delete = golem.DefineMethod[Id, ObjectIn, golem.Unit]("delete",
		golem.Desc("Delete an object"))
	// List returns the container's object names, sorted so the result is comparable.
	List = golem.DefineMethod[Id, ContainerIn, []string]("list",
		golem.Desc("List a container's objects, sorted"))
)
