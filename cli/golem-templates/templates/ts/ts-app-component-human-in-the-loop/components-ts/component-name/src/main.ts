import {
  BaseAgent,
  agent,
  prompt,
  description,
} from '@golemcloud/golem-ts-sdk';
import { createPromise, awaitPromise, completePromise, PromiseId } from 'golem:api/host@1.1.7';

type WorkflowId = string;

// --- Workflow Agent ---
@agent()
class ApprovalWorkflow extends BaseAgent {
  private readonly workflowId: WorkflowId;

  constructor(workflowId: WorkflowId) {
    super();
    this.workflowId = workflowId;
  }

  @prompt("Start approval process")
  @description("Starts a workflow that requires human approval before continuing")
  async start(): Promise<string> {
    // 1. Create a promise that represents waiting for human input
    const approvalPromiseId = createPromise();

    // Normally you would send this ID to some UI, email, etc.
    // For demo purposes we’ll just tell you where to send the approval
    const approver = HumanAgent.get("bob");
    await approver.requestApproval(this.workflowId, approvalPromiseId);

    // 2. Pause here until promise is completed
    const resultBytes = awaitPromise(approvalPromiseId);
    const result = new TextDecoder().decode(resultBytes);

    // 3. Continue based on human input
    if (result === "approved") {
      return `Workflow ${this.workflowId} was approved ✅`;
    } else {
      return `Workflow ${this.workflowId} was rejected ❌`;
    }
  }
}

// --- Human Agent ---
@agent()
class HumanAgent extends BaseAgent {
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
  async listPendingApprovals(): Promise<string[]> {
    return Array.from(this.pending.keys());
  }

  @prompt("Approve or reject a workflow")
  @description("Makes a decision on a workflow approval request")
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
