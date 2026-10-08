// Package task is the DEFINITION of a task manager showing structured I/O: the
// method inputs and results are Go structs, so the whole Task record
// round-trips across the wire. The agent also mounts an HTTP surface, one
// endpoint per method.
package task

import "github.com/golemcloud/golem/sdks/go/golem"

type ID struct{ Name string }

type Task struct {
	Id        int64
	Title     string
	Completed bool
	CreatedAt string
}

type CreateTaskIn struct{ Title string }

// CompleteTaskIn's Id is bound from the request path.
type CompleteTaskIn struct{ Id int64 }

var Agent = golem.DefineAgent[ID](golem.Spec{
	Name:        "TaskAgent",
	Description: "A task manager with JSON object I/O",
	HTTP:        &golem.Mount{Path: "/task-agents/{name}", CORS: []string{"*"}},
})

var (
	// CreateTask creates a task from a JSON body `{"title": …}` and returns the
	// full Task.
	CreateTask = Agent.Method[CreateTaskIn, Task]("createTask",
		golem.Desc("Create a task"),
		golem.HTTP(golem.POST("/tasks")))
	// GetTasks lists every task as a JSON array.
	GetTasks = Agent.Method[golem.Unit, []Task]("getTasks",
		golem.Desc("List every task"),
		golem.HTTP(golem.GET("/tasks")))
	// CompleteTask marks a task completed by id; it returns none for an unknown
	// id.
	CompleteTask = Agent.Method[CompleteTaskIn, golem.Option[Task]]("completeTask",
		golem.Desc("Mark a task completed"),
		golem.HTTP(golem.POST("/tasks/{id}/complete")))
)
