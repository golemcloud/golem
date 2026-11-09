package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt
import cloud.golem.annotations.RemoteAgent
import cloud.golem.runtime.PromiseId
import cloud.golem.runtime.Promises
import cloud.golem.runtime.SchemaValue

/** Typed client for [HumanAgent]; KSP generates `HumanAgentClientRpc` from it. */
@RemoteAgent(typeName = "HumanAgent")
interface HumanAgentClient {
    fun requestApproval(workflowId: String, promiseId: PromiseId): String
}

/**
 * A workflow that needs a human's approval before it continues. It creates a promise, asks the approver to complete it and suspends until they do:
 * the wait is durable, so the agent survives restarts while it is blocked.
 *
 * Mounted at /workflows/{workflowId}.
 */
@Agent(mount = "/workflows/{workflowId}", description = "A workflow that waits for a human decision")
class WorkflowAgent(val workflowId: String) : BaseAgent() {
    @Prompt("Start approval process")
    @Description("Starts a workflow that requires human approval before continuing")
    @Endpoint(post = "/start")
    fun start(approver: String): String {
        val promiseId = Promises.create()
        val human = HumanAgentClientRpc(SchemaValue.Record(listOf(SchemaValue.Str(approver))))
        try {
            human.requestApproval(workflowId, promiseId)
        } finally {
            human.close()
        }
        val decision = Promises.await(promiseId).decodeToString()
        return if (decision == "approved") "Workflow $workflowId was approved ✅" else "Workflow $workflowId was rejected ❌"
    }
}
