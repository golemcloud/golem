package cloud.golem

import cloud.golem.runtime.selfAgentId
import cloud.golem.runtime.selfAgentType
import cloud.golem.runtime.selfAgentName

internal actual fun currentAgentId(): String = selfAgentId()
internal actual fun currentAgentType(): String = selfAgentType()
internal actual fun currentAgentName(): String = selfAgentName()
