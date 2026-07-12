package cloud.golem.ksp

import kotlin.test.Test
import kotlin.test.assertNull
import kotlin.test.assertTrue

class HttpValidationTest {

    private fun param(name: String) = ParamModel(name, TypeDesc.Prim("string"))

    private fun method(
        name: String,
        params: List<String> = emptyList(),
        endpoints: List<HttpEndpointModel> = emptyList(),
    ) = MethodModel(
        name = name,
        description = "",
        promptHint = "",
        inputParams = params.map { param(it) },
        outputTypeDesc = TypeDesc.Prim("s32"),
        httpEndpoints = endpoints,
    )

    private fun model(
        mount: String,
        ctorParams: List<String> = emptyList(),
        methods: List<MethodModel> = emptyList(),
    ) = AgentModel(
        className = "CounterAgent",
        qualifiedName = "counter.CounterAgent",
        packageName = "counter",
        mountPath = mount,
        classDescription = "",
        mountAuth = false,
        mountCors = emptyList(),
        mode = "durable",
        snapshotting = "disabled",
        constructorParams = ctorParams.map { param(it) },
        methods = methods,
    )

    @Test
    fun `the counter template validates`() {
        val m = model(
            mount = "/counters/{name}",
            ctorParams = listOf("name"),
            methods = listOf(
                method("increment", endpoints = listOf(HttpEndpointModel("POST", "/increment"))),
                method("getValue", endpoints = listOf(HttpEndpointModel("GET", "/value"))),
            ),
        )
        assertNull(HttpValidation.validate(m))
    }

    @Test
    fun `an unmounted agent with no endpoints is valid`() {
        assertNull(HttpValidation.validate(model(mount = "", ctorParams = listOf("name"))))
    }

    @Test
    fun `catch-all in the mount path is rejected`() {
        val err = HttpValidation.validate(model(mount = "/files/{+rest}", ctorParams = listOf("rest")))
        assertTrue(err != null && err.contains("catch-all"), "got: $err")
    }

    @Test
    fun `mount variable without a matching constructor param is rejected`() {
        val err = HttpValidation.validate(model(mount = "/counters/{id}", ctorParams = listOf("name")))
        assertTrue(err != null && err.contains("'{id}'") && err.contains("constructor parameter"), "got: $err")
    }

    @Test
    fun `constructor param not provided by the mount path is rejected`() {
        val err = HttpValidation.validate(model(mount = "/counters/{name}", ctorParams = listOf("name", "region")))
        assertTrue(err != null && err.contains("region") && err.contains("not provided"), "got: $err")
    }

    @Test
    fun `endpoint on an unmounted agent is rejected`() {
        val m = model(
            mount = "",
            methods = listOf(method("increment", endpoints = listOf(HttpEndpointModel("POST", "/increment")))),
        )
        val err = HttpValidation.validate(m)
        assertTrue(err != null && err.contains("no HTTP mount"), "got: $err")
    }

    @Test
    fun `endpoint path variable must be a method parameter`() {
        val m = model(
            mount = "/orders/{name}",
            ctorParams = listOf("name"),
            methods = listOf(
                method("item", params = listOf("sku"), endpoints = listOf(HttpEndpointModel("GET", "/items/{itemId}"))),
            ),
        )
        val err = HttpValidation.validate(m)
        assertTrue(err != null && err.contains("'{itemId}'") && err.contains("item"), "got: $err")
    }

    @Test
    fun `endpoint path variable bound to a method parameter is valid`() {
        val m = model(
            mount = "/orders/{name}",
            ctorParams = listOf("name"),
            methods = listOf(
                method("item", params = listOf("itemId"), endpoints = listOf(HttpEndpointModel("GET", "/items/{itemId}"))),
            ),
        )
        assertNull(HttpValidation.validate(m))
    }

    @Test
    fun `catch-all is allowed in an endpoint suffix when bound to a method parameter`() {
        val m = model(
            mount = "/fs/{name}",
            ctorParams = listOf("name"),
            methods = listOf(
                method("read", params = listOf("rest"), endpoints = listOf(HttpEndpointModel("GET", "/read/{+rest}"))),
            ),
        )
        assertNull(HttpValidation.validate(m))
    }
}
