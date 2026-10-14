// Package impl is the IMPLEMENTATION of the task agent. Importing it registers
// the agent.
package impl

import (
	"time"

	"component-name/agents/task"

	"github.com/golemcloud/golem/sdks/go/golem"
)

type state struct {
	tasks  []task.Task
	nextID int64
}

var agent = task.Agent.Implement(func(task.ID) *state { return &state{nextID: 1} })

func init() {
	agent.Handle(task.CreateTask, func(ctx *golem.Context[state], in task.CreateTaskIn) task.Task {
		t := task.Task{
			Id:        ctx.State.nextID,
			Title:     in.Title,
			CreatedAt: time.Now().UTC().Format(time.RFC3339),
		}
		ctx.State.nextID++
		ctx.State.tasks = append(ctx.State.tasks, t)
		return t
	})
	agent.Handle(task.GetTasks, func(ctx *golem.Context[state], _ golem.Unit) []task.Task {
		return ctx.State.tasks
	})
	agent.Handle(task.CompleteTask, func(ctx *golem.Context[state], in task.CompleteTaskIn) golem.Option[task.Task] {
		for i := range ctx.State.tasks {
			if ctx.State.tasks[i].Id == in.Id {
				ctx.State.tasks[i].Completed = true
				return golem.Some(ctx.State.tasks[i])
			}
		}
		return golem.None[task.Task]()
	})
}
