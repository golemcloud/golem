import { Effect, Ref, Schema } from "effect";
import { defineAgent, Http, method } from "@golemcloud/effect-golem";

const Task = Schema.Struct({
  id: Schema.Number,
  title: Schema.String,
  completed: Schema.Boolean,
  createdAt: Schema.String,
});
type Task = typeof Task.Type;

export const TaskAgent = defineAgent({
  name: "TaskAgent",
  id: { name: Schema.String },
  http: Http.mount("/task-agents/{name}", { cors: ["*"] }),
  methods: {
    createTask: method({
      input: { request: Schema.Struct({ title: Schema.String }) },
      success: Task,
      http: [Http.post("/tasks")],
    }),
    getTasks: method({
      input: {},
      success: Schema.Array(Task),
      http: [Http.get("/tasks")],
    }),
    completeTask: method({
      input: { id: Schema.Number },
      success: Schema.NullOr(Task),
      http: [Http.post("/tasks/{id}/complete")],
    }),
  },
}).implement({
  init: () => Ref.make({ tasks: [] as Task[], nextId: 1 }),
  methods: (state) => ({
    createTask: ({ request }) =>
      Effect.gen(function* () {
        const createdAt = yield* Effect.sync(() => new Date().toISOString());
        return yield* Ref.modify(state, ({ tasks, nextId }) => {
          const task: Task = {
            id: nextId,
            title: request.title,
            completed: false,
            createdAt,
          };
          return [task, { tasks: [...tasks, task], nextId: nextId + 1 }];
        });
      }),
    getTasks: () => Ref.get(state).pipe(Effect.map(({ tasks }) => tasks)),
    completeTask: ({ id }) =>
      Ref.modify(state, ({ tasks, nextId }) => {
        const task = tasks.find((task) => task.id === id);
        if (!task) return [null, { tasks, nextId }];
        const completed = { ...task, completed: true };
        return [
          completed,
          {
            tasks: tasks.map((task) => (task.id === id ? completed : task)),
            nextId,
          },
        ];
      }),
  }),
});
