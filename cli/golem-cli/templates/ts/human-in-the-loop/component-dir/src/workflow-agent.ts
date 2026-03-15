import {
  BaseAgent,
  agent,
  prompt,
  description,
  createPromise,
  awaitPromise,
  endpoint
} from '@golemcloud/golem-ts-sdk';
import { HumanAgent, WorkflowId } from './human-agent';

@agent({
  mount: '/workflows',
  // The agent identity is not fully described by the constructor parameters,
  // so we need to enable phantom mode to allow duplicates.
  phantom: true,
})
export class WorkflowAgent extends BaseAgent {
  private readonly workflowId: WorkflowId;

  constructor() {
    super();
    this.workflowId = crypto.randomUUID();
  }

  @prompt("Start approval process")
  @description("Starts a workflow that requires human approval before continuing")
  @endpoint({ post: "/" })
  async start(): Promise<string> {
    // 1. Create a promise that represents waiting for human input
    const approvalPromiseId = createPromise();

    // Normally you would send this ID to some UI, email, etc.
    // For demo purposes we’ll just tell you where to send the approval
    const approver = HumanAgent.get("bob");
    await approver.requestApproval(this.workflowId, approvalPromiseId);

    // 2. Pause here until promise is completed
    const resultBytes = await awaitPromise(approvalPromiseId);
    const result = new TextDecoder().decode(resultBytes);

    // 3. Continue based on human input
    if (result === "approved") {
      return `Workflow ${this.workflowId} was approved ✅`;
    } else {
      return `Workflow ${this.workflowId} was rejected ❌`;
    }
  }
}
