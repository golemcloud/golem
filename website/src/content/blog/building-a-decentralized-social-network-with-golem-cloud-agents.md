---
title: "Building a Decentralized Social Network with Golem Cloud Agents"
date: "2026-06-19"
author: "Peter Kotula"
tags: ["Industry Articles"]
slug: "building-a-decentralized-social-network-with-golem-cloud-agents"
originalUrl: "https://golem.cloud/post/building-a-decentralized-social-network-with-golem-cloud-agents"
---

## Introduction

In the evolving landscape of cloud computing, the concept of **agents** is reshaping how we think about distributed systems. Instead of monolithic backends or stateless microservices reliant on external databases, we can now build applications where data and logic coexist within autonomous, persistent entities.

In this post, we'll explore **Golem Social Net**, a proof-of-concept social networking application built on **Golem Cloud**. We'll dive into its agentic architecture, communication flows, and how specific agents are implemented in Rust.

## Overview: The Agentic Paradigm

Traditional web architectures often separate the "compute" (stateless servers) from the "storage" (databases). Golem Cloud challenges this by introducing **Stateful Workers** (Agents). An agent in Golem is a durable execution unit that maintains its own state in memory, which persists across restarts and upgrades.

In our social network, we don't have a central "User Database" or "Post Table". Instead:
- Every **User** is an independent agent.
- Every **Post** is an independent agent.
- Every **Chat** is an independent agent.

This granular approach allows for natural scalability and fault isolation. If one user's agent fails (which Golem handles gracefully), it doesn't bring down the entire system.

## Architecture

The system is composed of a constellation of agents, split into two main categories: **Stateful Agents** (persistent entities) and **Ephemeral Agents** (view aggregators and searchers).

![Architecture Diagram](/blog-images/social-net-architecture.png)
*Figure 1: High-level Architecture of Golem Social Net*

### Communication Flow

The system manages interactions through a mix of synchronous RPC calls and asynchronous invocations:

1.  **Request Entry**: The **API Gateway** receives REST requests and routes them to the appropriate agent (e.g., `GET /users/{id}` → `User Agent {id}`) based on the HTTP‑endpoint definitions (mount paths) of each agent and the `httpApi` configuration in `golem.yaml`.

2.  **User Registration**: When a new `User Agent` is created for the first time, it automatically determines the appropriate shard using MD5 hashing and triggers the corresponding **User Index Agent** shard to register its ID, ensuring the sharded registry is always up-to-date.

3.  **Efficient Search**: A **User Search Agent** queries all **User Index Agent** shards in parallel to get the complete list of user IDs, then fetches user profiles in parallel chunks for optimal performance using improved async handling.

4.  **Fan-out Distribution**: When a user creates a post:
    *   The **User Posts Agent** initializes a new **Post Agent**.
    *   The **Post Agent** asynchronously invokes the **Timelines Updater Agent**. This is a durable, guaranteed operation enhanced by improved async reliability.
    *   The **Timelines Updater Agent** looks up the author's followers and "fans out" the post reference to their personal **User Timeline Agents**.

5.  **View Aggregation**: To show a timeline, a **User Timeline View Agent** queries the `User Timeline Agent` for a list of post IDs, then fetches the actual content from multiple `Post Agents` in parallel, constructing a complete view using optimized concurrent execution.

## Agent Design

The system's logic is distributed across several specialized agents. Here is a complete breakdown of every agent type in the application.

### 1. Core Entity Agents

These agents represent the primary domain entities.

#### User Agent
The **User Agent** is the persistent identity of a user. It stores profile data and manages the list of connections (friends and followers).

```rust
#[agent_definition(mount = "/v1/social-net/users/{id}")]
trait UserAgent {
    fn new(id: String) -> Self;
    
    #[endpoint(get = "/")]
    fn get_user(&self) -> Option<User>;
    
    fn get_user_if_match(&self, query: query::Query) -> Option<User>;
    
    #[endpoint(put = "/name")]
    fn set_name(&mut self, name: Option<String>) -> Result<UpdateResponse, ErrorResponse>;
    
    #[endpoint(put = "/connections")]
    fn connect_user(&mut self, user_id: String, connection_type: UserConnectionType) -> Result<UpdateResponse, ErrorResponse>;
    // ... disconnect, etc.
}
```

#### Post Agent
The **Post Agent** manages the lifecycle of a single post, including its content, likes, and a tree of comments. It is also responsible for triggering updates when its state changes.

```rust
#[agent_definition(mount = "/v1/social-net/posts/{id}")]
trait PostAgent {
    fn new(id: String) -> Self;
    
    #[endpoint(get = "/")]
    fn get_post(&self) -> Option<Post>;
    
    fn get_post_if_match(&self, query: query::Query) -> Option<Post>;
    
    async fn init_post(&mut self, user_id: String, content: String) -> Result<(), String>;
    
    #[endpoint(post = "/comments")]
    fn add_comment(&mut self, content: String, user_id: String, parent_comment_id: Option<String>) -> Result<AddCommentResponse, ErrorResponse>;
    
    #[endpoint(put = "/likes")]
    fn set_like(&mut self, user_id: String, like_type: LikeType) -> Result<UpdateResponse, ErrorResponse>;
}
```

Here we can see how the `Post Agent` proactively notifies the `Timelines Updater Agent` upon creation:

```rust
async fn init_post(&mut self, user_id: String, content: String) -> Result<(), String> {
    // ... setup state ...
    TimelinesUpdaterAgentClient::get(user_id.clone())
        .trigger_post_updated(PostUpdate::from(state), true);
    // ...
}
```

#### Chat Agent
The **Chat Agent** handles a single chat room. It stores the message history and participation list.

```rust
#[agent_definition(mount = "/v1/social-net/chats/{id}")]
trait ChatAgent {
    fn new(id: String) -> Self;
    
    #[endpoint(get = "/")]
    fn get_chat(&self) -> Option<Chat>;
    
    fn get_chat_if_match(&self, query: query::Query) -> Option<Chat>;
    
    #[endpoint(post = "/messages")]
    fn add_message(&mut self, content: String, user_id: String) -> Result<AddMessageResponse, ErrorResponse>;
    
    #[endpoint(put = "/participants")]
    fn add_participants(&mut self, participants: HashSet<String>) -> Result<UpdateResponse, ErrorResponse>;
    
    fn init_chat(&mut self, participants_ids: HashSet<String>, created_by: String, created_at: chrono::DateTime<chrono::Utc>) -> Result<(), String>;
}
```

When a message is added, the `Chat Agent` iterates through all participants and updates their individual `User Chats Agent` registries, ensuring their chat lists move to the top:

```rust
fn execute_chat_updates(
    chat_id: String,
    participants_ids: HashSet<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
) {
    for p_id in participants_ids {
        UserChatsAgentClient::get(p_id.clone())
            .trigger_chat_updated(chat_id.clone(), updated_at);
    }
}
```

### 2. Collection & Registry Agents

These stateful agents manage collections of references, provide centralized registry services, and link core entities together.

#### User Index Agent
The **User Index Agent** serves as a sharded registry for all users in the system, providing a scalable source of truth for user discovery and search operations. This implementation leverages enhanced agent capabilities for improved performance and reliability.

```rust
#[agent_definition]
trait UserIndexAgent {
    fn new(shard_id: u32) -> Self;
    fn add(&mut self, user_id: String) -> bool;
    fn get_state(&self) -> UserIndexState;
}
```

**Sharding Implementation**: MD5-based consistent hashing distributes users across multiple shards, each shard validates users belong to it before accepting, and search operations query all shards in parallel using optimized concurrent execution.

```rust
const USER_INDEX_SHARDS: u32 = 8;

pub fn get_user_index_shard(user_id: &str) -> u32 {
    get_shard_number(user_id.to_string(), USER_INDEX_SHARDS)
}

// Shard validation in add method
fn add(&mut self, user_id: String) -> bool {
    let expected_shard = get_user_index_shard(&user_id);
    if expected_shard == self.shard_id {
        self.state.add_user(user_id)
    } else {
        false // Reject users that don't belong to this shard
    }
}
```

This agent maintains a durable set of all user IDs with automatic registration when new users are created. It eliminates the need for expensive agent discovery operations by providing the User Search Agent with a complete list of users through efficient parallel shard queries.

These stateful agents manage collections of references, linking core entities together.

#### User Posts Agent
This agent acts as a registry for all posts created by a specific user. It generates unique IDs for new posts and delegates the actual creation to a fresh `Post Agent`.

```rust
#[agent_definition(mount = "/v1/social-net/users/{id}")]
trait UserPostsAgent {
    fn new(id: String) -> Self;

    #[endpoint(get = "/posts")]
    fn get_posts(&self) -> Option<UserPosts>;

    #[endpoint(post = "/posts")]
    fn create_post(&mut self, content: String) -> Result<PostRef, ErrorResponse>;

    fn get_updates(&self, updates_since: chrono::DateTime<chrono::Utc>)
    -> Option<UserPostsUpdates>;
}
```

The `create_post` function demonstrates how distinct agents are orchestrated. It generates an ID, initializes a specific `Post Agent`, and then stores the reference locally:

```rust
fn create_post(&mut self, content: String) -> Result<PostRef, ErrorResponse> {
    self.with_state(|state| {
        let post_id = uuid::Uuid::new_v4().to_string();

        let post_ref = PostRef::new(post_id.clone());

        PostAgentClient::get(post_id.clone())
            .trigger_init_post(state.user_id.clone(), content);

        state.updated_at = post_ref.created_at;
        state.posts.push(post_ref.clone());

        Ok(post_ref)
    })
}
```

#### User Timeline Agent
This agent maintains the personal timeline for a user. It stores references (`PostRef`) to posts from friends and followed users. It receives updates via the fan-out mechanism.

```rust
#[agent_definition]
trait UserTimelineAgent {
    fn new(id: String) -> Self;

    fn get_timeline(&self) -> Option<UserTimeline>;

    fn posts_updated(&mut self, posts: Vec<PostRef>) -> Result<(), String>;

    fn get_updates(
        &self,
        updates_since: chrono::DateTime<chrono::Utc>,
    ) -> Option<UserTimelineUpdates>;
}
```

#### User Chats Agent
Similar to `User Posts Agent`, this registry tracks all chat rooms a user is a participant in.

```rust
#[agent_definition(mount = "/v1/social-net/users/{id}")]
trait UserChatsAgent {
    fn new(id: String) -> Self;

    #[endpoint(get = "/chats")]
    fn get_chats(&self) -> Option<UserChats>;

    #[endpoint(post = "/chats")]
    fn create_chat(&mut self, participants: HashSet<String>) -> Result<ChatRef, ErrorResponse>;

    fn add_chat(
        &mut self,
        chat_id: String,
        created_by: String,
        created_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String>;

    fn chat_updated(
        &mut self,
        chat_id: String,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String>;

    fn get_updates(&self, updates_since: chrono::DateTime<chrono::Utc>)
    -> Option<UserChatsUpdates>;
}
```

Triggering a new chat involves a similar pattern: creating the ID, initializing the dedicated `Chat Agent` with participants, and updating the local list.

```rust
fn create_chat(&mut self, participants: HashSet<String>) -> Result<ChatRef, ErrorResponse> {
    self.with_state(|state| {
        let u_id = state.user_id.clone();
        let participants_ids: HashSet<String> = participants
            .into_iter()
            .filter(|id| id.clone() != u_id)
            .collect::<HashSet<_>>();
        if participants_ids.is_empty() {
            Err(ErrorResponse {
                message: "Chat must have at least 2 participants".to_string(),
            })
        } else {
            let chat_id = uuid::Uuid::new_v4().to_string();

            let chat_ref = ChatRef::new(chat_id.clone(), u_id);
            let created_at = chat_ref.created_at;

            ChatAgentClient::get(chat_id.clone()).trigger_init_chat(
                participants_ids,
                state.user_id.clone(),
                created_at,
            );

            state.chats.push(chat_ref.clone());
            state.updated_at = created_at;

            Ok(chat_ref)
        }
    })
}
```

### 3. Orchestration & Updates Agents

These agents handle background processing and real-time updates.

#### Timelines Updater Agent
This is the "fan-out" worker. When a post is created, this agent determines who needs to see it and pushes the update to their respective `User Timeline Agents`.

```rust
#[agent_definition]
trait TimelinesUpdaterAgent {
    fn new(id: String) -> Self;
    async fn post_updated(&mut self, update: PostUpdate, process_immediately: bool);
    async fn process_posts_updates(&mut self);
}
```

The `post_updated` method receives the update. If `process_immediately` is true, it triggers the fan-out right away. Otherwise, it buffers the update.

```rust
 async fn post_updated(&mut self, update: PostUpdate, process_immediately: bool) {
    self.add_update(update);

    if process_immediately {
        self.execute_posts_updates().await;
    }
}
```

The `execute_posts_updates` (helper function) demonstrates the logic of finding followers and pushing data to them:

```rust
async fn execute_posts_updates(user_id: String, updates: Vec<PostUpdate>) -> bool {
    // 1. Fetch the author's profile to get connections
    let user = UserAgentClient::get(user_id.clone()).get_user().await;

    if let Some(user) = user {
        // 2. Identify followers and friends
        let mut notify_user_ids: HashMap<String, UserConnectionType> = HashMap::new();
        // ... filter connected_users ...

        // 3. Push updates to each follower's timeline
        for (connected_user_id, connection_type) in notify_user_ids {
            let user_updates = updates.clone().into_iter().map(|update| {
                 // ... create PostRef ...
            }).collect();
            
            UserTimelineAgentClient::get(connected_user_id)
                .trigger_posts_updated(user_updates);
        }
        true
    } else {
        false
    }
}
```

#### User Timeline Updates Agent (Ephemeral)
This ephemeral agent implements a **long-polling** mechanism. It checks the `User Timeline Agent` for any changes since a specific timestamp, allowing the frontend to receive real-time updates without constant refreshing.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserTimelineUpdatesAgent {
    fn new() -> Self;
    
    #[endpoint(get = "/{user_id}/timeline/posts/updates?since={since}&iter-wait-time={iter_wait_time}&max-wait-time={max_wait_time}")]
    async fn get_posts_updates(
        &mut self,
        user_id: String,
        since: Option<String>,
        iter_wait_time: Option<u32>,
        max_wait_time: Option<u32>,
    ) -> Option<Vec<PostRef>>;
}
```

The implementation leverages a generic `poll_for_updates` helper to handle the loop and timeout logic, keeping the agent code clean and focused on the specific data retrieval:

```rust
async fn get_posts_updates(
    &mut self,
    user_id: String,
    since: Option<String>,
    iter_wait_time: Option<u32>,
    max_wait_time: Option<u32>,
) -> Option<Vec<PostRef>> {
    // Convert string timestamp to DateTime if provided
    let updates_since = since.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    
    poll_for_updates(
        user_id,
        updates_since,
        iter_wait_time,
        max_wait_time,
        |uid, since_dt| async move {
            let res = UserTimelineAgentClient::get(uid).get_updates(since_dt).await;
            res.map(|r| r.posts)
        },
        "get posts updates",
    )
    .await
}
```

Here is the generic `poll_for_updates` function that encapsulates the polling logic:

```rust
pub async fn poll_for_updates<T, F, Fut>(
    user_id: String,
    updates_since: Option<chrono::DateTime<chrono::Utc>>,
    iter_wait_time: Option<u32>,
    max_wait_time: Option<u32>,
    get_updates_fn: F,
    log_prefix: &str,
) -> Option<Vec<T>>
where
    F: Fn(String, chrono::DateTime<chrono::Utc>) -> Fut,
    Fut: std::future::Future<Output = Option<Vec<T>>>,
{
    let since = updates_since.unwrap_or(chrono::Utc::now());
    let max_wait_time = Duration::from_millis(max_wait_time.unwrap_or(10000) as u64);
    let iter_wait_time = Duration::from_millis(iter_wait_time.unwrap_or(1000) as u64);
    let now = Instant::now();
    let mut done = false;
    let mut result: Option<Vec<T>> = None;

    while !done {
        log::info!(
            "{} - user id: {}, updates since: {}, elapsed time: {}ms, max wait time: {}ms",
            log_prefix,
            user_id,
            since,
            now.elapsed().as_millis(),
            max_wait_time.as_millis()
        );

        let res = get_updates_fn(user_id.clone(), since).await;

        if let Some(updates) = res {
            if !updates.is_empty() {
                result = Some(updates);
                done = true;
            } else {
                result = Some(vec![]);
                done = now.elapsed() >= max_wait_time;
                if !done {
                    thread::sleep(iter_wait_time);
                }
            }
        } else {
            result = None;
            done = true;
        }
    }
    result
}
```

#### User Chats Updates Agent (Ephemeral)
Similar to the timeline updater, this agent provides long-polling for the user's chat list, ensuring they see new conversations immediately.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserChatsUpdatesAgent {
    fn new() -> Self;
    
    #[endpoint(get = "/{user_id}/chats/updates?since={since}&iter-wait-time={iter_wait_time}&max-wait-time={max_wait_time}")]
    async fn get_chats_updates(
        &mut self,
        user_id: String,
        since: Option<String>,
        iter_wait_time: Option<u32>,
        max_wait_time: Option<u32>,
    ) -> Option<Vec<ChatRef>>;
}
```

### 4. View & Discovery Agents (Ephemeral)

These agents are stateless aggregators. They query multiple stateful agents in parallel to build complete views for the frontend.

#### User Search Agent
This agent provides efficient user search by querying all **User Index Agent** shards in parallel for user IDs, then fetching user profiles in parallel chunks. This approach eliminates expensive agent discovery operations and provides scalable search performance using enhanced async capabilities.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserSearchAgent {
    fn new() -> Self;
    
    #[endpoint(get = "/search?query={query}")]
    async fn search(&self, query: String) -> Result<Vec<User>, ErrorResponse>;
}
```

**Search Implementation**: The agent queries all User Index Agent shards concurrently using optimized parallel execution, aggregates user IDs from all shards before filtering, and processes them in parallel chunks for performance.

```rust
async fn search(&self, query: String) -> Result<Vec<User>, ErrorResponse> {
    let query_obj = query::Query::new(&query);
    
    // Query all shards in parallel using enhanced concurrency
    let shard_futures: Vec<_> = (0..USER_INDEX_SHARDS)
        .map(|shard_id| async move {
            UserIndexAgentClient::get(shard_id).get_state().await
        })
        .collect();
    
    let shard_states = join_all(shard_futures).await;
    
    // Collect all user IDs
    let mut all_user_ids = HashSet::new();
    for state in shard_states {
        all_user_ids.extend(state.user_ids);
    }
    
    // Filter and fetch users
    let ids = all_user_ids
        .into_iter()
        .filter(|id| matches_query(id.clone(), &query_obj))
        .collect::<HashSet<_>>();

    let users = get_users_filtered(ids, query_obj).await?;
    Ok(users)
}
```

#### User Posts View Agent
It aggregates data by getting a list of post IDs from `User Posts Agent` and then fetching full content from each `Post Agent` in parallel using the `get_post_if_match` method.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserPostsViewAgent {
    fn new() -> Self;

    #[endpoint(get = "/{user_id}/posts/search?query={query}")]
    async fn get_posts_view(&mut self, user_id: String, query: String) -> Option<Vec<Post>>;

    async fn get_posts_updates_view(
        &mut self,
        user_id: String,
        updates_since: chrono::DateTime<chrono::Utc>,
    ) -> Option<Vec<Post>>;
}
```

This agent leverages the  `fetch_posts_by_ids_and_query` function:

```rust
async fn get_posts_view(&mut self, user_id: String, query: String) -> Option<Vec<Post>> {
    let user_posts = UserPostsAgentClient::get(user_id.clone()).get_posts().await;

    if let Some(user_posts) = user_posts {
        let query = query::Query::new(&query);
        let user_posts = user_posts.posts;
        
        if user_posts.is_empty() {
            Some(vec![])
        } else {
            let post_ids: Vec<String> = user_posts.iter().map(|p| p.post_id.clone()).collect();
            let posts = fetch_posts_by_ids_and_query(&post_ids, query).await;
            Some(posts)
        }
    } else {
        None
    }
}
```

#### User Timeline View Agent
This agent is similar to the **User Posts View Agent**, but it aggregates the user's timeline using the `fetch_posts_by_ids_and_query` function.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserTimelineViewAgent {
    fn new() -> Self;

    #[endpoint(get = "/{user_id}/timeline/posts?query={query}")]
    async fn get_posts_view(&mut self, user_id: String, query: String) -> Option<Vec<Post>>;

    async fn get_posts_updates_view(
        &mut self,
        user_id: String,
        since: Option<String>,
    ) -> Option<Vec<Post>>;
}
```

#### User Chats View Agent
This agent aggregates full chat states for the user's chat list using the `fetch_chats_by_ids_and_query` function.

```rust
#[agent_definition(mode = "ephemeral", mount = "/v1/social-net/users")]
trait UserChatsViewAgent {
    fn new() -> Self;

    #[endpoint(get = "/{user_id}/chats/search?query={query}")]
    async fn get_chats_view(&mut self, user_id: String, query: String) -> Option<Vec<Chat>>;

    async fn get_chats_updates_view(
        &mut self,
        user_id: String,
        since: Option<String>,
    ) -> Option<Vec<Chat>>;
}
```

## Frontend

While the backend infrastructure is a network of agents, the frontend is a web application.

Built with **Vue 3**, **TypeScript**, and **Tailwind CSS**, it communicates with the Golem agents via a standard REST API exposed by the Golem Gateway. The frontend doesn't need to know it's talking to thousands of distributed agents; it just makes HTTP requests like any other SPA. All the complexity of routing to the correct agent is handled by the Golem infrastructure.

---

**Conclusion**

Golem Social Net demonstrates that complex, stateful applications can be built without managing databases or monolithic application servers. By treating every entity as an autonomous agent, we gain a system that is naturally modular, scalable, and resilient.

The combination of the agent-based architecture provides a powerful foundation for building sophisticated distributed applications that can scale efficiently while maintaining clean separation of concerns and excellent developer experience.


## Next Steps

1. Explore the [GitHub repository](https://github.com/justcoon/golem-social-net-rust)
2. Try deploying your own instance
3. Contribute to the project
4. Check out the [TypeScript implementation](https://github.com/justcoon/golem-social-net-ts) for a similar application

## Resources

- [Golem Documentation](https://learn.golem.cloud/)
- [Rust Programming Language](https://www.rust-lang.org/)
- [WebAssembly](https://webassembly.org/)
