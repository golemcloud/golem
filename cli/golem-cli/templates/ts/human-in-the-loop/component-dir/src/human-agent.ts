import {
  BaseAgent,
  agent,
  prompt,
  description,
  PromiseId,
  completePromise,
  endpoint
} from '@golemcloud/golem-ts-sdk';

export type WorkflowId = string;

@agent({
  mount: "/humans/{username}"
})
export class HumanAgent extends BaseAgent {
  private readonly username: string;
  private pending: Map<WorkflowId, PromiseId> = new Map();

  constructor(username: string) {
    super();
    this.username = username;
  }

  @prompt("Receive approval request")
  @description("Stores a pending approval request from a workflow")
  async requestApproval(workflowId: WorkflowId, promiseId: PromiseId): Promise<string> {
    this.pending.set(workflowId, promiseId);
    return `User ${this.username} received approval request for workflow ${workflowId}`;
  }

  @prompt("List pending approvals")
  @description("Lists all workflows that are waiting for this human’s approval")
  @endpoint({ get: "/pending" })
  async listPendingApprovals(): Promise<string[]> {
    return Array.from(this.pending.keys());
  }

  @prompt("Approve or reject a workflow")
  @description("Makes a decision on a workflow approval request")
  @endpoint({ post: "/decisions" })
  async decideApproval(workflowId: string, decision: string): Promise<string> {
    if (!["approved", "rejected"].includes(decision)) {
      return `Received invalid approval decision ${decision}`
    }

    const promiseId = this.pending.get(workflowId);
    if (!promiseId) {
      return `No pending request found for workflow ${workflowId}`;
    }

    completePromise(promiseId, new TextEncoder().encode(decision));
    this.pending.delete(workflowId);

    return `Workflow ${workflowId} was ${decision} by ${this.username}`;
  }
}
