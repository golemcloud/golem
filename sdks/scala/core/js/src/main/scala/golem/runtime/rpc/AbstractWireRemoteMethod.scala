package golem.runtime.rpc

import golem.Datetime
import golem.runtime.WireClientMethod
import scala.concurrent.Future

abstract class AbstractWireRemoteMethod[Trait, In, Out] protected (
  resolved: AgentClientRuntime.WireResolvedAgent[Trait],
  methodName: String
) {
  protected final lazy val method: WireClientMethod[Trait] { type Input = In; type Output = Out } =
    resolved.methodByName[In, Out](methodName)
  protected final def awaitWith(input: In): Future[Out]                           = resolved.await(method, input)
  protected final def awaitWithMetadata(input: In): Future[InvocationResult[Out]] =
    resolved.awaitWithMetadata(method, input)
  protected final def triggerWith(input: In): Future[Invocation[Out]]                  = resolved.trigger(method, input)
  protected final def scheduleWith(input: In, when: Datetime): Future[Invocation[Out]] =
    resolved.schedule(method, when, input)
}
