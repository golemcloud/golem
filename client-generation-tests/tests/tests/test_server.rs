use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use warp::{Filter, Reply};
use serde::{Serialize, Deserialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestUser {
    pub id: u64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResponse {
    pub status: String,
    pub data: Option<TestUser>,
}

type Users = Arc<RwLock<HashMap<u64, TestUser>>>;

pub struct TestServer {
    users: Users,
}

impl TestServer {
    pub fn new() -> Self {
        Self {
            users: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn start(self, port: u16) {
        let users = self.users.clone();

        // GET /users/{id}
        let get_user = warp::path!("users" / u64)
            .and(warp::get())
            .and(with_users(users.clone()))
            .and_then(handle_get_user);

        // POST /users
        let create_user = warp::path("users")
            .and(warp::post())
            .and(warp::body::json())
            .and(with_users(users.clone()))
            .and_then(handle_create_user);

        // PUT /users/{id}
        let update_user = warp::path!("users" / u64)
            .and(warp::put())
            .and(warp::body::json())
            .and(with_users(users.clone()))
            .and_then(handle_update_user);

        // DELETE /users/{id}
        let delete_user = warp::path!("users" / u64)
            .and(warp::delete())
            .and(with_users(users.clone()))
            .and_then(handle_delete_user);

        let routes = get_user
            .or(create_user)
            .or(update_user)
            .or(delete_user)
            .with(warp::cors().allow_any_origin());

        warp::serve(routes).run(([127, 0, 0, 1], port)).await;
    }
}

fn with_users(users: Users) -> impl Filter<Extract = (Users,), Error = std::convert::Infallible> + Clone {
    warp::any().map(move || users.clone())
}

async fn handle_get_user(id: u64, users: Users) -> Result<impl Reply, warp::Rejection> {
    let users = users.read().await;
    match users.get(&id) {
        Some(user) => Ok(warp::reply::json(&TestResponse {
            status: "success".to_string(),
            data: Some(user.clone()),
        })),
        None => Ok(warp::reply::json(&TestResponse {
            status: "error".to_string(),
            data: None,
        })),
    }
}

async fn handle_create_user(new_user: TestUser, users: Users) -> Result<impl Reply, warp::Rejection> {
    let mut users = users.write().await;
    users.insert(new_user.id, new_user.clone());
    Ok(warp::reply::json(&TestResponse {
        status: "success".to_string(),
        data: Some(new_user),
    }))
}

async fn handle_update_user(id: u64, updated_user: TestUser, users: Users) -> Result<impl Reply, warp::Rejection> {
    let mut users = users.write().await;
    if users.contains_key(&id) {
        users.insert(id, updated_user.clone());
        Ok(warp::reply::json(&TestResponse {
            status: "success".to_string(),
            data: Some(updated_user),
        }))
    } else {
        Ok(warp::reply::json(&TestResponse {
            status: "error".to_string(),
            data: None,
        }))
    }
}

async fn handle_delete_user(id: u64, users: Users) -> Result<impl Reply, warp::Rejection> {
    let mut users = users.write().await;
    if users.remove(&id).is_some() {
        Ok(warp::reply::json(&json!({
            "status": "success",
            "message": "User deleted successfully"
        })))
    } else {
        Ok(warp::reply::json(&json!({
            "status": "error",
            "message": "User not found"
        })))
    }
} 