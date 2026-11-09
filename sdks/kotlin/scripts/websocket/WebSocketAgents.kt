package p24

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.websocket.ReconstructionPolicy
import cloud.golem.websocket.WebSocketConnection
import cloud.golem.websocket.WebSocketException
import cloud.golem.websocket.WebSocketMessage

@Agent(mount = "/ws/{id}", description = "websocket client spike")
class WsAgent(val id: String) : BaseAgent() {
    @Endpoint(post = "/run")
    fun run(url: String, badUrl: String): String {
        val out = mutableListOf<String>()
        val c = WebSocketConnection.connect(url, listOf("x-test" to "1"), ReconstructionPolicy.REPORT_CONNECTION_LOSS)
        c.send("hello")
        out += "text=" + c.receive()
        c.send(WebSocketMessage.Binary(byteArrayOf(1, 2, 3, -1)))
        out += "bin=" + (c.receive() as WebSocketMessage.Binary).bytes.toList()
        out += "timeout=" + c.receive(150)
        c.send("bye")
        out += "bye=" + c.receive(2000)
        c.close(1000, "done")
        try {
            WebSocketConnection.connect(badUrl)
            out += "bad=connected"
        } catch (e: WebSocketException) {
            out += "bad=" + e.error::class.simpleName
        }
        return out.joinToString(" | ")
    }
}
