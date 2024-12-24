use test_api;
use test_api::{Configuration, DefaultApi};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Configuration::new("http://localhost:3000".to_string());
    let client = DefaultApi::new(config);

    // Test create user
    let new_user = TestUser {
        id: 1,
        name: "Test User".to_string(),
        email: "test@example.com".to_string(),
    };
    let response = client.create_user(new_user).await?;
    assert_eq!(response.status, "success");

    // Test get user
    let response = client.get_user(1).await?;
    assert_eq!(response.data.unwrap().name, "Test User");

    // Test update user
    let updated_user = TestUser {
        id: 1,
        name: "Updated User".to_string(),
        email: "test@example.com".to_string(),
    };
    let response = client.update_user(1, updated_user).await?;
    assert_eq!(response.data.unwrap().name, "Updated User");

    // Test delete user
    let response = client.delete_user(1).await?;
    assert_eq!(response.status, "success");

    Ok(())
}
