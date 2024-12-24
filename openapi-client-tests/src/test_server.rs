use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use axum::{
    routing::{get, post, put, delete},
    Router,
    extract::{Path, State, Json},
    response::IntoResponse,
};
use serde::{Serialize, Deserialize};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestUser {
    pub id: i32,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResponse<T> {
    pub status: String,
    pub data: T,
}

type Users = Arc<Mutex<HashMap<i32, TestUser>>>;

pub struct TestServer {
    users: Users,
}

impl TestServer {
    pub fn new() -> Self {
        Self {
            users: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn get_user(
        State(users): State<Users>,
        Path(id): Path<i32>,
    ) -> impl IntoResponse {
        let users = users.lock().unwrap();
        match users.get(&id) {
            Some(user) => Json(TestResponse {
                status: "success".to_string(),
                data: user.clone(),
            }),
            None => Json(TestResponse {
                status: "error".to_string(),
                data: TestUser {
                    id: 0,
                    name: String::new(),
                    email: String::new(),
                },
            }),
        }
    }

    async fn create_user(
        State(users): State<Users>,
        Json(user): Json<TestUser>,
    ) -> impl IntoResponse {
        let mut users = users.lock().unwrap();
        users.insert(user.id, user.clone());
        Json(TestResponse {
            status: "success".to_string(),
            data: user,
        })
    }

    async fn update_user(
        State(users): State<Users>,
        Path(id): Path<i32>,
        Json(user): Json<TestUser>,
    ) -> impl IntoResponse {
        let mut users = users.lock().unwrap();
        users.insert(id, user.clone());
        Json(TestResponse {
            status: "success".to_string(),
            data: user,
        })
    }

    async fn delete_user(
        State(users): State<Users>,
        Path(id): Path<i32>,
    ) -> impl IntoResponse {
        let mut users = users.lock().unwrap();
        users.remove(&id);
        Json(TestResponse {
            status: "success".to_string(),
            data: TestUser {
                id,
                name: String::new(),
                email: String::new(),
            },
        })
    }

    pub async fn start(&self, port: u16) {
        let app = Router::new()
            .route("/users/:id", get(Self::get_user))
            .route("/users", post(Self::create_user))
            .route("/users/:id", put(Self::update_user))
            .route("/users/:id", delete(Self::delete_user))
            .with_state(self.users.clone());

        let addr = format!("127.0.0.1:{}", port);
        let listener = TcpListener::bind(&addr).await.unwrap();
        println!("Test server listening on {}", addr);

        axum::serve(listener, app).await.unwrap();
    }
}
