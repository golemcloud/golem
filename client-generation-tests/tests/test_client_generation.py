import pytest
import json
import httpx
import pytest_asyncio
from test_server import TestServer, User

def create_api_definition():
    """Create a test API definition matching Rust's HttpApiDefinition structure."""
    return {
        "id": "test-api",
        "version": "1.0.0",
        "routes": [
            {
                "method": "GET",
                "path": "/users/{id}",
                "binding": {
                    "component": None,
                    "worker_name": None
                },
                "cors": None,
                "security": None
            },
            {
                "method": "POST",
                "path": "/users",
                "binding": {
                    "component": None,
                    "worker_name": None
                },
                "cors": None,
                "security": None
            },
            {
                "method": "PUT",
                "path": "/users/{id}",
                "binding": {
                    "component": None,
                    "worker_name": None
                },
                "cors": None,
                "security": None
            },
            {
                "method": "DELETE",
                "path": "/users/{id}",
                "binding": {
                    "component": None,
                    "worker_name": None
                },
                "cors": None,
                "security": None
            }
        ],
        "draft": True,
        "security": None
    }

def generate_openapi_spec(api_def):
    """Generate OpenAPI specification from API definition."""
    paths = {}
    
    for route in api_def["routes"]:
        path = route["path"]
        method = route["method"].lower()
        
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
    """Test that our API definition matches the expected OpenAPI format."""
    # Create API definition
    api_def = create_api_definition()
    
    # Generate OpenAPI spec
    openapi_spec = generate_openapi_spec(api_def)
    
    # Verify OpenAPI spec structure
    assert openapi_spec["openapi"] == "3.0.0"
    assert openapi_spec["info"]["title"] == "Test API"
    assert openapi_spec["info"]["version"] == api_def["version"]
    
    # Verify paths
    paths = openapi_spec["paths"]
    assert "/users/{id}" in paths
    assert "/users" in paths
    
    # Verify methods
    user_id_path = paths["/users/{id}"]
    assert "get" in user_id_path
    assert "put" in user_id_path
    assert "delete" in user_id_path
    
    users_path = paths["/users"]
    assert "post" in users_path
    
    # Verify response schema
    get_response = user_id_path["get"]["responses"]["200"]
    assert get_response["description"] == "Success"
    assert "application/json" in get_response["content"]
    
    schema = get_response["content"]["application/json"]["schema"]
    assert schema["type"] == "object"
    assert "status" in schema["properties"]
    assert "data" in schema["properties"]
    
    data_schema = schema["properties"]["data"]
    assert data_schema["type"] == "object"
    assert "id" in data_schema["properties"]
    assert "name" in data_schema["properties"]
    assert "email" in data_schema["properties"]

@pytest.mark.asyncio
async def test_api_endpoints(server):
    """Test that the API endpoints work as expected."""
    async with httpx.AsyncClient(base_url="http://localhost:3000") as client:
        # Create user
        new_user = User(id=1, name="Test User", email="test@example.com")
        response = await client.post("/users", json=new_user.model_dump())
        assert response.status_code == 200
        assert response.json()["status"] == "success"
        
        # Get user
        response = await client.get("/users/1")
        assert response.status_code == 200
        data = response.json()["data"]
        assert data["name"] == "Test User"
        assert data["email"] == "test@example.com"
        
        # Update user
        updated_user = User(id=1, name="Updated User", email="test@example.com")
        response = await client.put("/users/1", json=updated_user.model_dump())
        assert response.status_code == 200
        assert response.json()["data"]["name"] == "Updated User"
        
        # Delete user
        response = await client.delete("/users/1")
        assert response.status_code == 200
        assert response.json()["status"] == "success"
        
        # Verify user is deleted
        response = await client.get("/users/1")
        assert response.status_code == 404
  