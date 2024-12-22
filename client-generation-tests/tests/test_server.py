from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from typing import Dict, Optional
import uvicorn
import asyncio
from contextlib import asynccontextmanager

class User(BaseModel):
    id: int
    name: str
    email: str

class Response(BaseModel):
    status: str
    data: Optional[User] = None

class TestServer:
    def __init__(self):
        self.users: Dict[int, User] = {}
        self.app = FastAPI()
        self._setup_routes()
        self.server = None

    def _setup_routes(self):
        @self.app.get("/users/{user_id}")
        async def get_user(user_id: int) -> Response:
            if user_id not in self.users:
                raise HTTPException(status_code=404, detail="User not found")
            return Response(status="success", data=self.users[user_id])

        @self.app.post("/users")
        async def create_user(user: User) -> Response:
            self.users[user.id] = user
            return Response(status="success", data=user)

        @self.app.put("/users/{user_id}")
        async def update_user(user_id: int, user: User) -> Response:
            if user_id != user.id:
                raise HTTPException(status_code=400, detail="ID mismatch")
            self.users[user_id] = user
            return Response(status="success", data=user)

        @self.app.delete("/users/{user_id}")
        async def delete_user(user_id: int) -> Response:
            if user_id not in self.users:
                raise HTTPException(status_code=404, detail="User not found")
            user = self.users.pop(user_id)
            return Response(status="success", data=user)

    async def start(self, host: str = "127.0.0.1", port: int = 3000):
        config = uvicorn.Config(self.app, host=host, port=port)
        self.server = uvicorn.Server(config)
        await self.server.serve()

    async def stop(self):
        if self.server:
            self.server.should_exit = True
            await self.server.shutdown()

    @asynccontextmanager
    async def run_server(self, host: str = "127.0.0.1", port: int = 3000):
        server_task = asyncio.create_task(self.start(host, port))
        # Give the server time to start
        await asyncio.sleep(1)
        try:
            yield self
        finally:
            await self.stop()
            server_task.cancel()
            try:
                await server_task
            except asyncio.CancelledError:
                pass