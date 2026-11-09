package p22

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.HttpHandler
import cloud.golem.annotations.HttpRouter
import cloud.golem.annotations.OpenApiProvider
import cloud.golem.http.HttpCodec
import cloud.golem.http.HttpHandlers
import cloud.golem.http.HttpHeader
import cloud.golem.http.HttpMiddleware
import cloud.golem.http.HttpRequest
import cloud.golem.http.HttpResponse
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc
import cloud.golem.streams.AgentStream

private var cleanups = 0

private val addHeader: HttpMiddleware = { next ->
    { request ->
        val response = next(request)
        HttpResponse(response.status, response.headers + HttpHeader.ascii("x-middleware", "on"), response.body)
    }
}

private fun core(request: HttpRequest): HttpResponse {
    val chunks = request.body.toList()
    val body = chunks.fold(ByteArray(0)) { a, b -> a + b }
    val head = "echo:${request.method} ${request.path}${request.query?.let { "?$it" } ?: ""} (${body.size} bytes, ${request.headers.size} headers) "
    return if (request.path.endsWith("/big")) {
        HttpResponse(200, listOf(HttpHeader.ascii("content-type", "application/octet-stream")), AgentStream.produce { sink ->
            for (i in 0 until 50) if (!sink.write(ByteArray(1000) { (i % 251).toByte() })) break
        })
    } else {
        HttpResponse(
            200,
            listOf(HttpHeader.ascii("content-type", "text/plain"), HttpHeader.ascii("set-cookie", "a=1"), HttpHeader.ascii("set-cookie", "b=2")),
            AgentStream.of(listOf(head.encodeToByteArray(), body)),
        )
    }
}

@HttpRouter(typeName = "Echo", mount = "/echo")
class EchoRouter : BaseAgent() {
    private val handler = HttpHandlers.ensuring(HttpHandlers.withMiddleware(::core, listOf(addHeader))) { cleanups++ }

    @HttpHandler
    fun serve(request: HttpRequest): HttpResponse = handler(request)

    @OpenApiProvider
    fun api(): String = """{"openapi":"3.1.0","info":{"title":"echo","version":"1"},"paths":{}}"""
}

/** The same handler on a durable agent: streams may only be attached between durable agents, and the router itself is ephemeral. */
@Agent(mount = "/echo-agent/{id}", description = "the router's handler as an ordinary method")
class EchoAgent(val id: String) : BaseAgent() {
    private val handler = HttpHandlers.ensuring(HttpHandlers.withMiddleware(::core, listOf(addHeader))) { cleanups++ }

    @Endpoint(post = "/serve")
    fun serve(request: HttpRequest): HttpResponse = handler(request)

    @Endpoint(post = "/cleanups")
    fun cleanupCount(): Int = cleanups
}

private const val RESPONSE_WIT = "record<status:u16,headers:list<record<name:string,value:list<u8>>>,body:stream<list<u8>>>"

@Agent(mount = "/http-caller/{id}", description = "calls the router over RPC")
class HttpCaller(val id: String) : BaseAgent() {
    @Endpoint(post = "/run")
    fun run(): String {
        val rpc = WasmRpc("EchoAgent", SchemaValue.Record(listOf(SchemaValue.Str("e1"))))
        val out = mutableListOf<String>()
        try {
            for ((path, body) in listOf("/echo/hello" to "hello body", "/echo/empty" to "", "/echo/big" to "x")) {
                val request = HttpRequest(
                    "POST", "https", "example.com", path, "a=1&b=2",
                    listOf(HttpHeader.ascii("x-one", "1"), HttpHeader.ascii("x-one", "2")),
                    AgentStream.of(if (body.isEmpty()) emptyList() else listOf(body.encodeToByteArray())),
                )
                val result = rpc.invokeAndAwait("serve", SchemaValue.Record(listOf(HttpCodec.encodeRequest(request))), RESPONSE_WIT)
                val value = (result as? cloud.golem.runtime.RpcResult.Ok)?.value
                if (value == null) {
                    out += "$path => $result"
                    continue
                }
                val response = HttpCodec.decodeResponse(value)
                val chunks = response.body.toList()
                val total = chunks.sumOf { it.size }
                val text = chunks.firstOrNull()?.decodeToString()?.take(60)
                out += "$path => ${response.status} headers=${response.headers.joinToString(",") { "${it.name}=${it.text}" }} chunks=${chunks.size} bytes=$total first='$text'"
            }
            val count = rpc.invokeAndAwait("cleanupCount", SchemaValue.Record(emptyList()), "s32")
            out += "cleanups=${(count as? cloud.golem.runtime.RpcResult.Ok)?.value}"
        } finally {
            rpc.close()
        }
        return out.joinToString(" | ")
    }
}
