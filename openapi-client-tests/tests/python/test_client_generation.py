import pytest
import httpx
import pytest_asyncio
from python.test_server import TestServer, User


def create_api_definition():
    """Create a test API definition matching the API structure."""
    return {
        "id": "test-api",
        "version": "1.0.0",
        "routes": [
            {
                "method": "GET",
                "path": "/users/{id}",
            },
            {
                "method": "POST",
                "path": "/users",
            },
            {
                "method": "PUT",
                "path": "/users/{id}",
            },
            {
                "method": "DELETE",
                "path": "/users/{id}",
            }
        ],
    }


def generate_openapi_spec(api_def):
    """Generate OpenAPI specification from the provided API definition."""
    paths = {}

    for route in api_def["routes"]:
        path = route["path"]
        method = route["method"].lower()

        # Define the operation and response schema
        operation = {
            "responses": {
                "200": {
                    "description": "Success",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "properties": {
                                    "status": {"type": "string"},
                                    "data": {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "integer"},
                                            "name": {"type": "string"},
                                            "email": {"type": "string"}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        # Insert the method and operation into the path
        if path not in paths:
            paths[path] = {}
        paths[path][method] = operation

    return {
        "openapi": "3.0.0",
        "info": {
            "title": "Test API",
            "version": api_def["version"]
        },
        "paths": paths
    }


@pytest_asyncio.fixture
async def server():
    """Fixture to start and stop the test server."""
    test_server = TestServer()
    async with test_server.run_server():
        yield test_server


@pytest.mark.asyncio
async def test_api_definition():
    """Test that the generated OpenAPI specification matches the definition."""
    api_def = create_api_definition()
    openapi_spec = generate_openapi_spec(api_def)

    # Verify OpenAPI structure
    assert openapi_spec["openapi"] == "3.0.0"
    assert openapi_spec["info"]["title"] == "Test API"
    assert openapi_spec["info"]["version"] == api_def["version"]

    paths = openapi_spec["paths"]
    assert "/users/{id}" in paths
    assert "/users" in paths

    # Verify response schema
    user_id_path = paths["/users/{id}"]
    assert "get" in user_id_path
    get_response = user_id_path["get"]["responses"]["200"]
    assert "application/json" in get_response["content"]

    schema = get_response["content"]["application/json"]["schema"]
    assert schema["type"] == "object"
    assert "data" in schema["properties"]


@pytest.mark.asyncio
async def test_api_endpoints(server):
    """Test that the API endpoints perform expected CRUD operations."""
    async with httpx.AsyncClient(base_url="http://localhost:3000") as client:
        # Create user
        new_user = User(id=1, name="Test User", email="test@example.com")
        response = await client.post("/users", json=new_user.dict())
        assert response.status_code == 200
        assert response.json()["status"] == "success"

        # Get user
        response = await client.get("/users/1")
        assert response.status_code == 200
        data = response.json()["data"]
        assert data["name"] == "Test User"

        # Update user
        updated_user = User(id=1, name="Updated User", email="test@example.com")
        response = await client.put("/users/1", json=updated_user.dict())
        assert response.status_code == 200
        assert response.json()["data"]["name"] == "Updated User"

        # Delete user
        response = await client.delete("/users/1")
        assert response.status_code == 200
        assert response.json()["status"] == "success"

        # Verify user is deleted
        response = await client.get("/users/1")
        assert response.status_code == 404
