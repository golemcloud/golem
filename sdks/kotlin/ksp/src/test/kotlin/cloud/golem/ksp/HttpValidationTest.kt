package cloud.golem.ksp

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
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

    private fun unstructured(name: String, binary: Boolean) = ParamModel(name, TypeDesc.UnstructuredT(binary = binary))

    private fun message(err: String?): String = err ?: error("expected a validation error")

    @Test
    fun `a phantom capture is not a constructor binding`() {
        val base = model(mount = "/literal/{customer}/{instance}", ctorParams = listOf("customer"))
        val path = base.copy(phantomBinding = Triple("path", "instance", true))
        assertNull(HttpValidation.validate(path))

        assertTrue(message(HttpValidation.validate(path.copy(mountPath = "/literal/{customer}/{instance}/{other}"))).contains("other"))

        val colliding = path.copy(constructorParams = listOf(param("customer"), param("instance")))
        assertTrue(message(HttpValidation.validate(colliding)).contains("constructor parameter"))

        assertTrue(message(HttpValidation.validate(path.copy(mountPath = "/literal/{customer}"))).contains("exactly one"))

        val query = base.copy(mountPath = "/literal/{customer}", phantomBinding = Triple("query", "instance", false))
        assertNull(HttpValidation.validate(query))
        assertTrue(HttpValidation.validate(query.copy(mountPath = "/literal/{customer}/{instance}")) != null)
    }

    @Test
    fun `an unstructured constructor parameter cannot be used with an HTTP mount`() {
        for ((binary, kind) in listOf(true to "UnstructuredBinary", false to "UnstructuredText")) {
            val m = model(mount = "/files/{blob}").copy(constructorParams = listOf(unstructured("blob", binary)))
            assertEquals(
                "Agent 'CounterAgent' constructor parameter 'blob' cannot be of type '$kind' when used with HTTP mount.",
                HttpValidation.validate(m),
            )
        }
    }

    @Test
    fun `header query and path variables cannot be bound to unstructured parameters`() {
        fun endpointOn(param: ParamModel, endpoint: HttpEndpointModel) = model(
            mount = "/orders/{name}",
            ctorParams = listOf("name"),
            methods = listOf(method("upload", endpoints = listOf(endpoint)).copy(inputParams = listOf(param))),
        )
        for ((binary, kind) in listOf(true to "UnstructuredBinary", false to "UnstructuredText")) {
            val p = unstructured("body", binary)
            val header = message(HttpValidation.validate(endpointOn(p, HttpEndpointModel("POST", "/upload", headerVars = listOf("X-Body" to "body")))))
            assertTrue(header.contains("header variable 'body'") && header.contains("'$kind'"), header)
            val query = message(HttpValidation.validate(endpointOn(p, HttpEndpointModel("POST", "/upload", queryVars = listOf("b" to "body")))))
            assertTrue(query.contains("query variable 'body'") && query.contains("'$kind'"), query)
            val path = message(HttpValidation.validate(endpointOn(p, HttpEndpointModel("POST", "/upload/{body}"))))
            assertTrue(path.contains("path variable 'body'") && path.contains("'$kind'"), path)
        }
    }

    @Test
    fun `an unstructured body parameter needs no binding`() {
        val m = model(
            mount = "/orders/{name}",
            ctorParams = listOf("name"),
            methods = listOf(method("upload", endpoints = listOf(HttpEndpointModel("POST", "/upload"))).copy(inputParams = listOf(unstructured("body", true)))),
        )
        assertNull(HttpValidation.validate(m))
    }

    @Test
    fun `a header variable that is not a method parameter is rejected`() {
        val m = model(
            mount = "/orders/{name}",
            ctorParams = listOf("name"),
            methods = listOf(method("item", params = listOf("x"), endpoints = listOf(HttpEndpointModel("GET", "/items", headerVars = listOf("X-Test" to "y"))))),
        )
        assertTrue(message(HttpValidation.validate(m)).contains("is not defined in method parameters"))
    }

    @Test
    fun `a valid endpoint binds path header and query variables to method parameters`() {
        val m = model(
            mount = "/foo/{name}",
            ctorParams = listOf("name"),
            methods = listOf(
                method(
                    "foo",
                    params = listOf("x", "h", "q"),
                    endpoints = listOf(HttpEndpointModel("GET", "/{x}", true, listOf("*"), listOf("X-Test" to "h"), listOf("q" to "q"))),
                ),
            ),
        )
        assertNull(HttpValidation.validate(m))
    }

    @Test
    fun `query bindings are parsed into parameter pairs`() {
        assertEquals(emptyList(), parseQueryBindings(""))
        assertEquals(listOf("x" to "var"), parseQueryBindings("x={var}"))
        assertEquals(listOf("x" to "var", "y" to "another"), parseQueryBindings("x={var}&y={another}"))
        assertEquals(listOf("foo" to "f", "bar" to "b"), parseQueryBindings("foo={f}&bar={b}"))
    }

    @Test
    fun `malformed query bindings are rejected with the reason`() {
        fun reason(query: String) = assertFailsWith<IllegalArgumentException> { parseQueryBindings(query) }.message.orEmpty()
        assertTrue(reason("x=").contains("Invalid query segment"))
        assertTrue(reason("={var}").contains("Invalid query segment"))
        assertEquals("Whitespace is not allowed in query variables", reason("x={ var }"))
        assertTrue(reason("x=var").contains("must be a variable reference"))
        assertTrue(reason("x={}").contains("cannot be an empty string"))
    }

    @Test
    fun `route templates parse into literal variable and catch-all segments`() {
        val segments = HttpValidation.parse("/foo/bar/{id}")
        assertEquals(3, segments.size)
        assertEquals(HttpValidation.Segment.Variable("id"), segments[2])
        assertEquals(HttpValidation.Segment.Remaining("rest"), HttpValidation.parse("/foo/{+rest}")[1])
    }
}
