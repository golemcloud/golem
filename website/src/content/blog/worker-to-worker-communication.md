---
title: "Worker-to-Worker Communication"
date: "2024-03-14"
# date sourced from site-deploy timestamp "Thu Mar 14 2024" embedded in first /post/ wayback snapshot (web.archive.org/web/20240407005816/https://www.golem.cloud/post/worker-to-worker-communication); post first appeared in blog index snapshot 2024-03-10, absent from 2024-03-01 snapshot
author: "Daniel Vigovszky"
tags: ["Engineering", "Golem Cloud", "WASM RPC", "Distributed Systems"]
slug: "worker-to-worker-communication"
originalUrl: "https://golem.cloud/post/worker-to-worker-communication"
---

# Worker-to-Worker Communication in Golem

Golem Cloud's first developer preview [has been unveiled in August](/blog/unveiling-golem-cloud), and just a month ago, we released [an open-source version of Golem](/blog/golem-goes-open-source). Workers, the fundamental primitive in Golem, expose a typed interface that can be invoked through the REST API or the command line tools, but until today, calling a worker from *another worker* was neither easy nor type-safe.

With the latest release of Golem and the `golem-cli` tool, we finally have a first-class, typed way to invoke one worker from another, using any of the supported guest languages!

## **Golem WASM RPC**

Golem's new worker to worker communication feature consists of two major layers:

- A low-level, dynamic worker invocation API exposed as a Golem **host function** to all workers. This interface is not type safe. Rather, it matches the capabilities of the external REST API, allowing a worker to invoke any method on any other worker with any parameters. However, it avoids the overhead of setting up an HTTP connection and will be optimized in the future.

- The ability to generate **stubs** for having a completely type-safe, language-independent remote worker invocation for any supported language having a WIT-based binding generator.

With the new stub generator commands integrated into Golem's command line tool (`golem-cli`) worker to worker communication is now a simple and fully type-safe experience.

## **A Full Example**

To demonstrate how this new feature works, we will take one of the first Golem example projects, the **shopping cart**, and extend it with worker-to-worker communication. The original shopping-cart project defines a worker for each shopping cart of an online web store, with exported functions to add items to the cart and eventually check out and finish the shopping process.

In this example, we introduce a second **worker template**, one that will be used to create a single **worker** for each online shopper. This worker will keep a log of all the purchases of the user it belongs to. We will extend the shopping cart's `checkout` function with a remote worker invocation to add a new entry to the account's purchase log.

First, let's make sure we have the latest version of `golem-cli`, if using the open-source Golem version, or `golem-cloud-cli`, if using the hosted version. It must have the new `stubgen` subcommand, to check let's run `golem-cli stubgen --help`:

```text
WASM RPC stub generator

Usage: golem-cli stubgen [OPTIONS] <COMMAND>

Commands:
  generate              Generate a Rust RPC stub crate for a WASM component
  build                 Build an RPC stub for a WASM component
  add-stub-dependency   Adds a generated stub as a dependency to another WASM component
  compose               Compose a WASM component with a generated stub WASM
  initialize-workspace  Initializes a Golem-specific cargo-make configuration in a Cargo workspace for automatically generating stubs and composing results
  help                  Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...  Increase logging verbosity
  -q, --quiet...    Decrease logging verbosity
  -h, --help        Print help
```

### **Preparing the Example**

We are going to create two different **Golem templates**, and have the source codes of both of them in a single **Cargo workspace**. This is not required—they could live in completely separate places—but it allows using our built-in cargo-make support, which currently gives us the best possible developer experience for worker-to-worker communication.

First, let's use the `golem-cli new` command to take the **shopping-cart example** and generate a new template source from it:

```bash
$ golem-cli new --example rust-shopping-cart --template-name shopping-cart-rpc
See the documentation about installing common tooling: https://golem.cloud/learn/rust

Compile the Rust component with cargo-component:
  cargo component build --release
The result in target/wasm32-wasi/release/shopping_cart_rpc.wasm is ready to be used with Golem!
```

The `shopping-cart-rpc` directory now contains a single Rust crate, which can be compiled to WASM using `cargo component build`. We need two different WASMs (two Golem templates) so as a first step, we convert the generated Cargo project to a [**cargo workspace**](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html).

First, create two sub-directories for the two templates we will use:

```bash
$ mkdir -pv shopping-cart
shopping-cart
$ mkdir -pv purchase-history
purchase-history
```

Then, move the generated shopping cart source code into the `shopping-cart` subdirectory:

```bash
$ mv -v src shopping-cart
src -> shopping-cart/src

$ mv -v wit shopping-cart
wit -> shopping-cart/wit

$ mv -v Cargo.toml shopping-cart
Cargo.toml -> shopping-cart/Cargo.toml
```

We can copy the whole contents of the `shopping-cart` directory to the `purchase-history` directory too:

```bash
$ cp -rv shopping-cart/* purchase-history
shopping-cart/Cargo.toml -> purchase-history/Cargo.toml
shopping-cart/src -> purchase-history/src
shopping-cart/src/lib.rs -> purchase-history/src/lib.rs
shopping-cart/wit -> purchase-history/wit
shopping-cart/wit/shopping-cart-rpc.wit -> purchase-history/wit/shopping-cart-rpc.wit
```

Then we create a new `Cargo.toml` file in the root, pointing to the two sub-projects:

```toml
[workspace]
resolver = "2"

members = [
    "shopping-cart",
    "purchase-history",
]
```

Next, modify the `name` property in both sub-project's `Cargo.toml`. In `shopping-cart/Cargo.toml`, it should be:

```toml
name = "shopping-cart"
```

while in the other

```toml
name = "purchase-history"
```

It's also recommended that you rename the WIT file in both the `wit` directories to a file name that corresponds to the given sub-project's name, but it does not have any effect on the compilation—it just makes working on the source code easier.

```bash
$ mv shopping-cart/wit/shopping-cart-rpc.wit shopping-cart/wit/shopping-cart.wit
$ mv purchase-history/wit/shopping-cart-rpc.wit purchase-history/wit/purchase-history.wit
```

At this point running `cargo component build` in the root will compile both identical sub-projects, creating two different WASM files (but both containing the shopping cart implementation for now):

```bash
$ cargo component build
...
    Creating component /Users/vigoo/projects/demo/shopping-cart-rpc/target/wasm32-wasi/debug/purchase_history.wasm
    Creating component /Users/vigoo/projects/demo/shopping-cart-rpc/target/wasm32-wasi/debug/shopping_cart.wasm
   
```

### **Implementing the Purchase History Template**

Before talking about *worker-to-worker communication*, let's just implement a simple version of the **purchase history template**. Each worker of this template will correspond to a **user** of the system, the worker name being equal to the user's identifier. We only need two exported functions, one for recording a purchase, and one for getting all the previous purchases.

Let's completely replace `purchase-history/wit/purchase-history.wit` with the following interface definition:

```wit
package shopping:purchase-history;

interface api {
  record product-item {
    product-id: string,
    name: string,
    price: float32,
    quantity: u32,
  }

  record order {
    order-id: string,
    items: list<product-item>,
    total: float32,
    timestamp: u64,
  }

  add-order: func(order: order) -> ();

  get-orders: func() -> list<order>;
}

world purchase-history {
  export api;
}
```

Our `product-item` and `order` types are the same that we have in the shopping-cart WIT. In a next step, we will remove them from the shopping-cart WIT, and import them from this component's interface definition!

Running `cargo component build` now will print a couple of errors, as we did not update the `purchase-history` module's Rust source code yet:

```text
$ cargo component build
...
error[E0433]: failed to resolve: could not find `golem` in `exports`
 --> purchase-history/src/lib.rs:3:31
  |
3 | use crate::bindings::exports::golem::template::api::*;
  |                               ^^^^^ could not find `golem` in `exports`
...
```

A simple implementation of this can be the following code replacing the existing `lib.rs`:

```rust
mod bindings;

use crate::bindings::exports::shopping::purchase_history::api::*;

struct Component;

struct State {
    orders: Vec<Order>,
}

static mut STATE: State = State {
    orders: Vec::new()
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

impl Guest for Component {
    fn add_order(order: Order) {
        with_state(|state| {
            state.orders.push(order);
        });
    }

    fn get_orders() -> Vec<Order> {
        with_state(|state| {
            state.orders.clone()
        })
    }
}
```

With this, `cargo component build` now compiles the new `purchase_history.wasm` for us.

### **Worker-to-Worker Communication**

At this point, the only outstanding task in our example is to **invoke the appropriate purchase history worker** in the `checkout` implementation of the shopping cart.

To find all the available options for doing this, check the [Worker-to-Worker communication's documentation](https://learn.golem.cloud/docs/rpc). In this example, we have both the target (the purchase history) and the caller (the shopping cart) in **the same cargo workspace**, so we can use Golem's [cargo-make](https://github.com/sagiegurari/cargo-make) based solution for enabling communication between the different sub-projects of the workspace.

Let's initialize this using `golem-cli` (or `golem-cloud-cli`):

```bash
$ golem-cli stubgen initialize-workspace --targets purchase-history --callers shopping-cart
Writing cargo-make Makefile to "/Users/vigoo/projects/demo/shopping-cart-rpc/Makefile.toml"
Generating initial stub for purchase-history
Generating stub WIT to /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history-stub/wit/_stub.wit
Copying root package shopping:purchasehistory
  .. /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history/wit/purchase-history.wit to /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history-stub/wit/deps/shopping_purchasehistory/purchase-history.wit
Writing wasm-rpc.wit to /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history-stub/wit/deps/wasm-rpc
Generating Cargo.toml to /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history-stub/Cargo.toml
Generating stub source to /Users/vigoo/projects/demo/shopping-cart-rpc/purchase-history-stub/src/lib.rs
Writing updated Cargo.toml to "/Users/vigoo/projects/demo/shopping-cart-rpc/Cargo.toml"
```

As a next step, we check if the generated artifacts work, by running **cargo make** to execute the full build flow. It contains custom steps invoking `golem-cli` to implement the typed worker-to-worker communication.

```bash
$ cargo make build-flow
...
    Creating component /Users/vigoo/projects/demo/shopping-cart-rpc/target/wasm32-wasi/debug/purchase_history.wasm
    Creating component /Users/vigoo/projects/demo/shopping-cart-rpc/target/wasm32-wasi/debug/shopping_cart.wasm
    Creating component /Users/vigoo/projects/demo/shopping-cart-rpc/target/wasm32-wasi/debug/purchase_history_stub.wasm
[cargo-make] INFO - Execute Command: "wasm-rpc-stubgen" "compose" "--source-wasm" "target/wasm32-wasi/debug/shopping_cart.wasm" "--stub-wasm" "target/wasm32-wasi/debug/purchase_history_stub.wasm" "--dest-wasm" "target/wasm32-wasi/debug/shopping_cart_composed.wasm"
Error: no dependencies of component `target/wasm32-wasi/debug/shopping_cart.wasm` were found
```

Don't worry about the failure at the end—it will be fixed in the next step.

There are several changes in our workspace after running this command:

- We have a `Makefile.toml` file describing custom build tasks related to worker-to-worker communication.

- We have a completely new sub-project called `purchase-history-stub`, which is added to the Cargo workspace.

- The `shopping-cart/wit/deps` directory now contains three dependencies: the original purchase history module, the generated stub interface, and the general-purpose `wasm-rpc` package.

- These dependencies are also registered in `shopping-cart/Cargo.toml`.

Before further explaining what these generated stubs are, let's finish our example. We need to modify the **shopping cart** template's interface definition (`shopping-cart/wit/shopping-cart.wit`) to import the generated stub, and to reuse the data types defined for the purchase history template instead of redefining them.

The updated WIT file would look like this:

```wit
package shopping:cart;

interface api {
  use shopping:purchase-history/api.{product-item};
  use shopping:purchase-history/api.{order};

  record order-confirmation {
    order-id: string,
  }

  variant checkout-result {
    error(string),
    success(order-confirmation),
  }

  initialize-cart: func(user-id: string) -> ();
  add-item: func(item: product-item) -> ();
  remove-item: func(product-id: string) -> ();
  update-item-quantity: func(product-id: string, quantity: u32) -> ();
  checkout: func() -> checkout-result;
  get-cart-contents: func() -> list<product-item>;
}

world shopping-cart {
  import shopping:purchase-history-stub/stub-purchase-history;
  export api;
}
```

There are three changes:

- We renamed the package from the default `golem:template` to `shopping:cart` to make it more consistent with the other packages

- We deleted the definition of `product-item` and `order`, and instead importing them from the `shopping:purchase-history` package.

- We added the `import` statement in the `world`, which loads the generated **stub** into the template's world, so we can call it from the Rust code to initiate remote calls to the `purchase-history` workers.

Because of the change of the package name, we have to update the import in `lib.rs` :

```rust
use crate::bindings::exports::shopping::cart::api::*;
```

The only remaining step is to extend the `checkout` function with the remote worker invocation!

```rust
use crate::bindings::shopping::purchase_history::api::{Order};
use crate::bindings::shopping::purchase_history_stub::stub_purchase_history;
use crate::bindings::golem::rpc::types::Uri;

fn checkout() -> CheckoutResult {
  // ...
    dispatch_order()?;

    // Defining the order to be saved in history
    let order = Order {
        items: state.items.clone(),
        order_id: order_id.clone(),
        timestamp: std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH).unwrap().as_secs(),
        total: state.items.iter().map(|item| item.price * item.quantity as f32).sum(),
    };

    // Constructing the remote worker's URI
    let template_id =
  		std::env::var("PURCHASE_HISTORY_TEMPLATE_ID")
  			.expect("PURCHASE_HISTORY_TEMPLATE_ID not set");
    let uri = Uri {
        value: format!("worker://{template_id}/{}", state.user_id),
    };

    // Connecdting to the remote worker and invoking it
    let history = stub_purchase_history::Api::new(&uri);
    history.add_order(&order);
}
```

With all these changes, running `cargo make` again will succeed:

```bash
$ cargo make build-flow
...
Writing composed component to "target/wasm32-wasi/debug/shopping_cart_composed.wasm"
[cargo-make] INFO - Build Done in 7.38 seconds.
```

We first created the `Order` value to be saved in the remote purchase history. Then we get an **environment variable** to figure out the Golem *template-id* of the purchase history template. This is something we need to record when uploading the template to Golem, and set it to all shopping cart workers when creating them. The remote URI consists of the template identifier and the *worker name*, and in our example the worker name is the same as the **user id** that the shopping cart belongs to. This guarantees that we will have a distinct purchase history worker for each user.

When we have the URI, we just instantiate the **generated stub** for by passing the remote worker's URI—and we get an interface that corresponds to the remote worker's exported interface! This way we can just call `add_order` on it, passing the constructed order value.

Everything else is handled by Golem. If this was the first order of the user, a new purchase history worker is created. Otherwise, the existing worker will be targeted, which is likely already in a suspended state, not actively in any worker executor's memory. Golem restores the worker's state and invokes the `add_order` function on them, which adds the new order to the list of orders for that user, in a fully durable way, without the need for a database.

### **How Does It Work?**

The generated cargo-make makefile just wraps a couple of `golem-cli stubgen` commands.

First, `stubgen generate` creates a new Rust crate for each **target** that has a similar interface as the original worker, but all the exported functions and interfaces are wrapped in a resource, which has to be instantiated with a **worker URI**. This generated crate can be compiled to a WASM file (or `stubgen build` can do that automatically) and it also contains a **WIT** file describing this interface.

The `stubgen add-stub-dependency` command takes this generated interface specification and **adds it** to an other worker's `wit` folder—making it a *dependency* of that worker. So the caller worker is not depending directly on the target worker, it depends on the **generated stub**.

If we compile this caller worker to WASM, it will not only require host functions provided by Golem (such as the WASI interfaces or Golem specific APIs) but it will also require an **implementation** of the stub interface. That's where the generated Rust crate comes into the picture—its compiled WASM **implements** (exports) the stub interface while the caller WASM **requires** (imports) it. WASM components can be composed so by combining the two we can get a result WASM that no longer tries to import the stub interface—it is going to be wired within the component—only the other dependencies the original modules had.

One way to do this composition is to use `wasm-tools compose`, but it is more convenient to use `golem-cli` (or `golem-cloud-cli`)'s built-in command for it, called `stubgen compose`. This is the last step the generated cargo-make file performs when running the `build-flow` task.

The following diagram demonstrates how the components in the example are interacting with each other:

![](/blog-images/68d76ba7edec7ec0b5c055eb_67559f0f16c5f7501359f4f8_Untitled.png)

## **Conclusion**

We have seen how the new Golem tools enable simple, fully-typed communication between **workers**. Although the above demonstrated `cargo-make`-based build is Rust specific, the other `stubgen` commands are not: they can be used with any language that has WIT binding generator support (see [Golem's Tier 2 languages](https://learn.golem.cloud/docs/building-templates/tier-2))—Rust, C, Go, JavaScript, Python and Scala.js.

The remote calls are not only simple to use, they are also efficient, and they get translated to direct function calls when the source and the target workers are running on the same **worker executor**. They are also fully durable, as all other external interaction running on Golem. This means we don't have to worry about failures when calling remote workers. Additionally, Golem applies retry policies in case of transient failures, and it makes sure that a remote invocation only happens once.

This feature is ready to use both in the [open source](http://github.com/golemcloud/golem) and the [cloud version](https://www.golem.cloud/).
