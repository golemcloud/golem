package p20

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.FileMapping
import cloud.golem.annotations.HttpRouter
import cloud.golem.annotations.OpenApiProvider
import cloud.golem.annotations.ResponseHeader
import cloud.golem.config.Config

data class SiteConfig(val title: String)

@HttpRouter(
    typeName = "Website",
    mount = "/web",
    staticBindings = [FileMapping("/", "/site/index.html"), FileMapping("/assets/*", "/site/\$1")],
    fileResponseHeaders = [ResponseHeader("Content-Security-Policy", "default-src 'self'"), ResponseHeader("Referrer-Policy", "same-origin")],
)
class Website(val config: Config<SiteConfig>) : BaseAgent() {
    @OpenApiProvider
    fun describe(): String = """{"openapi":"3.1.0","info":{"title":"${config.value.title}","version":"1"},"paths":{}}"""
}

@HttpRouter(typeName = "StaticOnly", mount = "/")
class StaticOnly : BaseAgent()

@Agent(
    mount = "/documents/{owner}",
    exposeFiles = [FileMapping("/latest", "/public/latest.txt"), FileMapping("/*", "/public/\$1")],
    fileResponseHeaders = [ResponseHeader("Referrer-Policy", "no-referrer")],
)
class Documents(val owner: String) : BaseAgent() {
    @Endpoint(post = "/update")
    fun update(text: String): String = "updated:$owner:$text"
}
