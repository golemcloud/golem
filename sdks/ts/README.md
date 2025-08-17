# golem-ts
`golem-ts` is a TypeScript library that provides high-level wrappers for Golem's runtime API, including the [transaction API]([https://learn.golem.cloud/docs/transaction-api](https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/transactions)), [durability controls](https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/durability) and customizing the [retry policy](https://learn.golem.cloud/docs/experimental-languages/ts-language-guide/retries). It simplifies the process of writing Golem programs by offering a set of utilities and abstractions.

## Installation

To install `golem-ts`, use the following command:

```bash
npm install @golemcloud/golem-ts
```

## Features

- **Transactions**: `golem-ts` supports both infallible and fallible transactions.
  - Use operations with compensations to handle failure cases gracefully.
- **Guards and Helpers**: The library provides guards and helper functions for various aspects of Golem programming.
  - Retry Policy: Define retry policies for operations to handle transient failures.
  - Idempotence Level: Specify the idempotence level of operations to ensure data consistency.
  - Persistence Level: Control the persistence level of operations to balance performance and durability.
  - Atomic Operations: Perform multiple operations atomically to maintain data integrity.
- **Result Type**: `golem-ts` introduces a `Result` type that enables typed errors, making error handling more robust and expressive.
- **Async to Sync**: utility functions for converting an async function into a synchronous one.
