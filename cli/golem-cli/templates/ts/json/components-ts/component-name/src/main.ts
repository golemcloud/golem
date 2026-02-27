import {
    BaseAgent,
    Result,
    agent,
    prompt,
    description,
    endpoint
} from '@golemcloud/golem-ts-sdk';

interface Task {
    id: number;
    title: string;
    completed: boolean;
    createdAt: string;
}

interface CreateTaskRequest {
    title: string;
}

@agent({
  mount: '/task-agents/{name}',
  cors: [ "*" ]
})
class TaskAgent extends BaseAgent {
    private tasks: Task[] = [];
    private nextId: number = 1;

    private readonly name: string;

    constructor(name: string) {
        super()
        this.name = name;
    }

    @prompt("Create a new task with the given title")
    @description("Creates a task and returns the complete task object")
    @endpoint({ post: "/tasks" })
    async createTask(request: CreateTaskRequest): Promise<Task> {
        const task: Task = {
            id: this.nextId++,
            title: request.title,
            completed: false,
            createdAt: new Date().toISOString()
        };

        this.tasks.push(task);
        return task;
    }

    @prompt("List all existing tasks")
    @description("Returns all tasks as a JSON array")
    @endpoint({ get: "/tasks" })
    async getTasks(): Promise<Task[]> {
        return this.tasks;
    }

  @description("Marks a task as completed by its ID")
  @endpoint({ post: "/tasks/{id}/complete" })
    async completeTask(id: number): Promise<Task | null> {
        const task = this.tasks.find(t => t.id === id);
        if (task) {
            task.completed = true;
            return task;
        }
        return null;
    }
}
