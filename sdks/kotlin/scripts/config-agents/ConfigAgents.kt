package p18

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.RemoteAgent
import cloud.golem.config.Config
import cloud.golem.config.ConfigOverride
import cloud.golem.config.Secret
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.TypedSchemaValue

data class DbConfig(val host: String, val port: Int, val password: Secret<String>)

data class AppConfig(
    val appName: String,
    val apiKey: Secret<String>,
    val db: DbConfig,
    val retries: Int?,
    val tags: List<String>?,
)

@Agent(mount = "/config/{id}", description = "Agent with configuration")
class ConfigAgent(val id: String, val config: Config<AppConfig>) : BaseAgent() {
    @Endpoint(post = "/greet")
    fun greet(): String {
        val cfg = config.value
        return "Hello from ${cfg.appName}! DB at ${cfg.db.host}:${cfg.db.port}, id=$id"
    }

    @Endpoint(post = "/secrets")
    fun secrets(): String {
        val cfg = config.value
        return "${cfg.apiKey.get()}|${cfg.db.password.get()}"
    }

    @Endpoint(post = "/optional")
    fun optional(): String {
        val cfg = config.value
        return "retries=${cfg.retries} tags=${cfg.tags}"
    }

    @Endpoint(post = "/secret-info")
    fun secretInfo(): String {
        val handle = config.value.apiKey.handle()
        try {
            val md = handle.metadata()
            return "id=${handle.id().size}B key=${md.configKey} category=${md.category} version=${md.version != null}"
        } finally {
            handle.release()
        }
    }
}

@RemoteAgent(typeName = "ConfigAgent")
interface ConfigAgentClient {
    fun greet(): String
}

@Agent(mount = "/config-caller/{id}", description = "Calls ConfigAgent with config overrides")
class ConfigCallerAgent(val id: String) : BaseAgent() {
    @Endpoint(post = "/override")
    fun callWithOverride(): String {
        val overrides = listOf(
            ConfigOverride(listOf("appName"), TypedSchemaValue("string", SchemaValue.Str("OverriddenApp"))),
            ConfigOverride(listOf("db", "host"), TypedSchemaValue("string", SchemaValue.Str("overridden-host.example.com"))),
            ConfigOverride(listOf("db", "port"), TypedSchemaValue("s32", SchemaValue.S32(9999))),
        )
        val client = ConfigAgentClientRpc(SchemaValue.Record(listOf(SchemaValue.Str(id))), overrides)
        try {
            return client.greet()
        } finally {
            client.close()
        }
    }
}
