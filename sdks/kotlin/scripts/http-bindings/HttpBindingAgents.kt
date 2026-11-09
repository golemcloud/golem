package p28

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Header

@Agent(mount = "/items/{owner}", description = "HTTP bindings")
class Items(val owner: String) : BaseAgent() {
    @Endpoint(get = "/find/{kind}?q={query}&limit={max}")
    fun find(kind: String, query: String, max: Int, @Header("X-Tenant") tenant: String): String = "$owner/$kind q=$query max=$max tenant=$tenant"

    @Endpoint(patch = "/rename")
    fun rename(to: String): String = "renamed:$to"

    @Endpoint
    fun rpcOnly(x: Int): Int = x + 1
}

@Agent(mount = "/ph/{id}", phantomIdQuery = "instance", phantomIdOptional = true)
class Phantomed(val id: String) : BaseAgent() {
    @Endpoint(get = "/who")
    fun who(): String = "phantomed:$id"
}

@Agent(mount = "/pp/{id}/{instance}", phantomIdPath = "instance")
class PathPhantom(val id: String) : BaseAgent() {
    @Endpoint(get = "/who")
    fun who(): String = "pathphantom:$id"
}
