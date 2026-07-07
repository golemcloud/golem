import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

// A task manager showing JSON object I/O in the fluent SDK: the method inputs and
// return values are Zod object schemas, so the whole `Task` record round-trips
// across the wire. The agent also mounts an HTTP surface — one endpoint per
// method via `http.mount(...)` on the agent and `method({ http: ... })`.
const Task = z.object({
  id: z.number(),
  title: z.string(),
  completed: z.boolean(),
  createdAt: z.string(),
});

type Task = z.infer<typeof Task>;

export const TaskAgent = defineAgent({
  name: 'TaskAgent',
  id: { name: z.string() },
  http: http.mount('/task-agents/{name}', { cors: ['*'] }),
  methods: {
    // Create a task from a JSON body `{ title }` and return the full Task object.
    createTask: method({
      input: { title: z.string() },
      returns: Task,
      http: http.post('/tasks'),
    }),
    // List every task as a JSON array.
    getTasks: method({
      input: {},
      returns: z.array(Task),
      http: http.get('/tasks'),
    }),
    // Mark a task completed by id (bound from the path); returns the Task or null.
    completeTask: method({
      input: { id: z.number() },
      returns: Task.nullable(),
      http: http.post('/tasks/{id}/complete'),
    }),
  },
});

export const TaskAgentImpl = TaskAgent.implement({
  init: () => ({ tasks: [] as Task[], nextId: 1 }),
  methods: {
    createTask({ title }) {
      const task: Task = {
        id: this.nextId++,
        title,
        completed: false,
        createdAt: new Date().toISOString(),
      };
      this.tasks.push(task);
      return task;
    },
    getTasks() {
      return this.tasks;
    },
    completeTask({ id }) {
      const task = this.tasks.find((t) => t.id === id);
      if (task) {
        task.completed = true;
        return task;
      }
      return null;
    },
  },
});
