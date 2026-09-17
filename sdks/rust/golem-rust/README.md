# golem-rust

A library that help writing [Golem](https://golem.cloud) programs by providing higher level Rust
wrappers for Golem's runtime APIs, including functions for defining and performing operations
transactionally.

## Retrying user code with semantic policies

Named policies are selected by the Golem host. The selected policy can be compiled into a local
schedule and applied to arbitrary async user code:

```rust
let raw = resolve_retry_policy("send", "email://welcome", &context)
    .expect("matching named policy");
let schedule = RetrySchedule::try_from(&raw)?;

schedule
    .retry_with_properties(
        async || send_email().await,
        |error| error.retry_properties(),
    )
    .await?;
```

To use a named policy definition without installing or resolving it through the host, compile its
inner policy directly:

```rust
let named = NamedPolicy::named("email", Policy::immediate().max_retries(3));
let raw = named.try_to_raw()?;
RetrySchedule::try_from(&raw.policy)?
    .retry(async || send_email().await)
    .await?;
```

`RetrySchedule` is a user-space guest loop. It does not install a policy or create executor
`RetryAttempt` entries, and its attempts are not one host-managed retry sequence. Host calls made
by the loop retain their normal durable replay and suspension behavior.
