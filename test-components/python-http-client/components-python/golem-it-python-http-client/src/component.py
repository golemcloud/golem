# make sure this stays before other urllib uses
from urllib3.contrib.wasi import enable_wasi_backend
enable_wasi_backend("golem_it_python_http_client")
from golem_it_python_http_client import exports
from urllib3.connection import HTTPConnection
import os

PORT = os.getenv("PORT")

class GolemItPythonHttpClientApi(exports.GolemItPythonHttpClientApi):
    def run(self) -> str:
        conn = HTTPConnection(f"localhost:{PORT}")
        conn.request(
            "POST",
            "/post-example",
            headers = {
                "x-test": f"test-header"
            },
            body = "test-body"
        )
        return conn.getresponse().json()
