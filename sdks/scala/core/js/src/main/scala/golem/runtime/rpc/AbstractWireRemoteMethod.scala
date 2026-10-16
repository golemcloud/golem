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
  protected final def awaitWith(input: In): Future[Out]                 = resolved.await(method, input)
  protected final def cancelableAwaitWith(input: In)                    = resolved.cancelableAwait(method, input)
  protected final def triggerWith(input: In)                            = resolved.trigger(method, input)
  protected final def scheduleWith(input: In, when: Datetime)           = resolved.schedule(method, when, input)
  protected final def scheduleCancelableWith(input: In, when: Datetime) =
    resolved.scheduleCancelable(method, when, input)
  protected final def awaitWithMetadata(input: In)                    = resolved.awaitWithMetadata(method, input)
  protected final def cancelableAwaitWithMetadata(input: In)          = resolved.cancelableAwaitWithMetadata(method, input)
  protected final def triggerWithMetadata(input: In)                  = resolved.triggerWithMetadata(method, input)
  protected final def scheduleWithMetadata(input: In, when: Datetime) =
    resolved.scheduleWithMetadata(method, when, input)
  protected final def scheduleCancelableWithMetadata(input: In, when: Datetime) =
    resolved.scheduleCancelableWithMetadata(method, when, input)
}
