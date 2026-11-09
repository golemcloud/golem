package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt
import cloud.golem.runtime.PromiseId
import cloud.golem.runtime.Promises

/**
 * A human approver. A workflow hands it the id of a promise it is waiting on; the human completes that promise by deciding.
 *
 * Mounted at /humans/{username}: each username is a separate agent with its own list of pending approvals.
 */
@Agent(mount = "/humans/{username}", description = "A human who approves or rejects workflows")
class HumanAgent(val username: String) : BaseAgent() {
    private val pending = LinkedHashMap<String, PromiseId>()

    @Description("Stores a pending approval request from a workflow")
    @Endpoint
    fun requestApproval(workflowId: String, promiseId: PromiseId): String {
        pending[workflowId] = promiseId
        return "User $username received approval request for workflow $workflowId"
    }

    @Description("Lists all workflows that are waiting for this human's approval")
    @Endpoint(get = "/pending")
    fun listPendingApprovals(): List<String> = pending.keys.toList()

    @Prompt("Approve or reject a workflow")
    @Description("Makes a decision on a workflow approval request")
    @Endpoint(post = "/decisions")
    fun decideApproval(workflowId: String, decision: String): String {
        if (decision != "approved" && decision != "rejected") return "Received invalid approval decision $decision"
        val promiseId = pending[workflowId] ?: return "No pending request found for workflow $workflowId"
        val completed = Promises.complete(promiseId, decision.encodeToByteArray())
        pending.remove(workflowId)
        return if (completed) "Workflow $workflowId was $decision by $username" else "Workflow $workflowId was already decided"
    }
}
