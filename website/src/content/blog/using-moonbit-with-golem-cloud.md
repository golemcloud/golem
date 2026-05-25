---
title: "Using MoonBit with Golem Cloud"
date: "2025-01-03"
author: "Daniel Vigovszky"
tags: ["MoonBit", "WebAssembly", "Golem Cloud", "Cloud Computing", "Distributed Systems"]
slug: "using-moonbit-with-golem-cloud"
originalUrl: "https://golem.cloud/post/using-moonbit-with-golem-cloud"
---

## Introduction

[MoonBit](https://www.moonbitlang.com/), a new programming language has been open sourced a few weeks ago - see [this blog post](https://www.moonbitlang.com/blog/compiler-opensource). MoonBit is an exciting modern programming language that natively supports WebAssembly, including the component model - this makes it a perfect fit for writing applications for [Golem Cloud](https://golem.cloud/).

In this post I'm exploring the current state of MoonBit and whether it is ready for writing Golem components, by implementing an example application more complex than a simple "hello world" example.

The application to be implemented is a simple collaborative list editor - on the [launch event of Golem 1.0](https://youtu.be/11Cig1iH6S0) I have live-coded the same example using three different programming languages (TypeScript, Rust and Go) for the three main modules it requires. In this post I am implementing all three using **MoonBit**, including the e-mail sending feature that was omitted from the live demo due to time constraints.

The application can handle an arbitrary number of simultaneously open **lists**. Each list consists of a list of string items. These items can be appended, inserted and deleted simultaneously by multiple users; the current list state can be queried any time, as well as the active connections (users who can perform editing operations on the list). Modification is only allowed for connected editors, and there is a poll function exposed for them which returns the new changes since the last poll call. Lists can be archived, in which case they are no longer editable and their contents are saved in a separate **list archive**. Then the list itself can be deleted, its last state remains stored forever in the archive. An additional feature is that if a list is *not archived* and there were no changes for a certain period of time, all the connected editors are notified by sending an **email** to them.

## Golem Architecture

In Golem a good architecture to run this is to have three different **golem components**:

- the list
- the archive
- the email notifier

These are compiled WebAssembly components, each exporting a distinct set of functions. Golem provides APIs to invoke these functions from the external world (for example mapping them to a HTTP API) and also allows **workers** (instances of these components) to invoke each other. A component can have an arbitrary number of instances, each such worker being identified by a unique name.

We can use this feature to have a very simple and straightforward implementation of the list editor - each document (editable list) will be mapped to its own worker, identified by the list's identifier. This way our list component only has to deal with a single list; scaling it up to handle multiple (possibly even millions) of lists is done automatically by Golem.

For archiving lists, we want to store each archived list in a single place - so we are going to have only a single instance of our archive component, where each archived list information is sent to. This singleton worker can store the archived lists in some database if needed - but because Golem's durable execution guarantees, it is enough to just store them in memory (one important exception is if we want to store a really large amount of archived lists not fitting in a single worker's memory). Golem guarantees that the worker's state is restored in any case of failure or rescaling event so the archive component can really remain very simple.

Finally, because Golem workers are single threaded and do not support async calls overlapping with its invocations at the moment, we need a third component to implement the delayed email sending functionality. There will be an **email sending worker** corresponding to each **list worker** and this worker will be suspended for an extended period of time (the amount we want to wait before sending out the email). Again, because of Golem's durable execution feature we can just "sleep" for an arbitrary long time in this component and we don't need to care about what can happen to our execution environment during that long period.

## Initial MoonBit implementation

Before going into details of how to develop Golem components with MoonBit, let's try to implement the above described components in this new language, without any Golem or WebAssembly specifics.

First we create a new lib project using moon new. This creates a new **project** with a single **package**. To match our architecture let's start by creating multiple packages, one for each component to develop (list, archive, email)

We create a folder for each package, with a moon.pkg.json in each:

```json
{
    "import": [
    ]
}
```

### List model

Let's start by modelling our **list**. The edited "document" itself is just an array of strings:

```moonbit
struct Document {
  mut items: Array[String]
}
```

We can implement **methods** on Document corresponding to the document editing operations we want to support. On this level we don't care about collaborative editing or connected users, just model our document as a pure data structure:

```moonbit
///| Creates an empty document
pub fn Document::new() -> Document {
  { items: [] }
}

///| Adds a new item to the document
pub fn add(self : Document, item : String) -> Unit {
  if self.items.search(item).is_empty() {
    self.items.push(item)
  }
}

///| Deletes an item from the document
pub fn delete(self : Document, item : String) -> Unit {
  self.items = self.items.filter(fn(i) { item != i })
}

///| Inserts an item to the document after an existing item. If `after` is not in the document, the new item is inserted at the end.
pub fn insert(self : Document, after~ : String, value~ : String) -> Unit {
  let index = self.items.search(after)
  match index {
    Some(index) => self.items.insert(index + 1, value)
    None => self.add(value)
  }
}

///| Gets a view of the document's items
pub fn get(self : Document) -> ArrayView[String] {
  self.items[:]
}

///| Iterates the items in the document
pub fn iter(self : Document) -> Iter[String] {
  self.items.iter()
}
```

We can also use MoonBit's built-in test feature to write unit tests for this. The following test contains an assertion that the initial document is empty:

```moonbit
test "new document is empty" {
  let empty = Document::new()
  assert_eq!(empty.items, [])
}
```

With the inspect function tests can use **snapshot values** to compare values with. The moon CLI tool and the IDE integration provides a way to automatically update the snapshot values (the content= part) in these test functions when needed:

```moonbit
test "basic document operations" {
  let doc = Document::new()
    ..add("x")
    ..add("y")
    ..add("z")
    ..insert(after="y", value="w")
    ..insert(after="a", value="b")
    ..delete("z")
    ..delete("f")
  inspect!(
    doc.get(),
    content=
      #|["x", "y", "w", "b"]
    ,
  )
}
```

### List editor state

The next step is to implement the editor state management on top of this Document type. As a reminder, we decided that every instance (Golem worker) of the list component will be only responsible for editing a single list. So we don't need to care about storing and indexing the lists, or routing connections to the corresponding node where the list state is - this is all going to be managed by Golem.

What we need to do, however, is write stateful code to handle connecting and disconnecting users ("editors"), adding some validation on top of the document editing API so only connected editors can make changes, and collect change events for the polling API.

We can start by defining a new datatype holding our document editing state:

```moonbit
///| Document state
struct State {
  document : Document
  connected : Map[ConnectionId, EditorState]
  mut last_connection_id : ConnectionId
  mut archived : Bool
  mut email_deadline : @datetime.DateTime
  mut email_recipients : Array[EmailAddress]
}
```

Beside the actual document we are going to store:

- A map of connected editors, with some per-editor state associated with them
- The last used connection ID so we can always generate a new unique one
- Whether the document has been archived or not
- When should we send out the email notification, and to what recipients

So far we have only defined the Document type so let's continue by specifying all these other types used in State's fields.

ConnectionId is going to be a **newtype** wrapping an integer:

```moonbit
///| Identifier of a connected editor
type ConnectionId Int derive(Eq, Hash)

///| Generates a next unique connection ID
fn next(self : ConnectionId) -> ConnectionId {
  ConnectionId(self._ + 1)
}
```

We want to use this type as a **key** of a Map so we need instances of the Eq and Hash type classes. MoonBit can derive it for us automatically for newtypes. In addition to that, we also define a method called next that generates a new connection ID with an incremented value.

The EditorState structure holds information for each connected editor. To keep things simple, we only store the editor's **email address** and a buffer of change events since the last call to poll.

An email address is a newtype of a String:

```moonbit
///| Email address of a connected editor
type EmailAddress String
```

The Change enum describes the possible changes made to the document:

```moonbit
///| An observable change of the edited document
enum Change {
  Added(String)
  Deleted(String)
  Inserted(after~ : String, value~ : String)
} derive(Show)
```

Deriving Show (or implementing it by hand) makes it possible to use the inspect test function to compare string snapshots of array of changes with the results of our poll function.

Finally, let's define EditorState using these two new types:

```moonbit
///| State per connected editor
struct EditorState {
  email : EmailAddress
  mut events : Array[Change]
}
```

The email field never changes of a connected editor - but the events array is, as every call to poll will reset this so the next poll returns only the new changes. To be able to do this, we have to mark it as mut-able.

The last new type we need to introduce for State is something representing a point in time. MoonBit's core standard library does not have currently anything for this, but there is already a package database, [mooncakes](https://mooncakes.io/), with published MoonBit packages. Here we can find a [package called datetime](https://mooncakes.io/docs/#/suiyunonghen/datetime/). Adding it to our project can be done with the moon CLI:

```bash
 moon add suiyunonghen/datetime
```

and then importing it into the list package by modifying its moon.pkg.json:

```json
{
    "import": [
        "suiyunonghen/datetime"
    ]
}
```

With this we can refer to the DateTime type in this package using @datetime.DateTime.

Before starting to implement methods for State, we have to think about error handling too - some of the operations on State may fail, for example if a wrong connection ID is used, or a document editing operation comes in for an already archived list. MoonBit has built-in support for error handling, and it starts by defining our own error type in the following way:

```moonbit
///| Error type for editor state operations
type! EditorError {
  ///| Error returned when an invalid connection ID is used
  InvalidConnection(ConnectionId)
  ///| Error when trying to modify an already archived document
  AlreadyArchived
}
```

With this we are ready to implement the collaborative list editor! I'm not going to list *all* the methods of State in this post, but the full source code is available [on GitHub](https://github.com/vigoo/golem-moonbit-example).

The connect method associates a new connection ID with the connected user, and also returns the current document state. This is important to be able to use the results of poll - the returned list of changes have to be applied to exactly this document state on the client side.

```moonbit
///| Connects a new editor
pub fn connect(
  self : State,
  email : EmailAddress
) -> (ConnectionId, ArrayView[String]) {
  let connection_id = self.last_connection_id.next()
  self.last_connection_id = connection_id
  self.connected.set(connection_id, EditorState::new(email))
  (connection_id, self.document.get())
}
```

The *editing operations* are more interesting. They build on top of the editing operations we already defined for Document, but in addition to that, they all perform the following tasks:

- Validating the connection ID
- Validating that the document is not archived yet
- Adding a Change event to each connected editor's state
- Updating the email_deadline and email_recipients fields, as each editing operation *resets* the timeout for sending out the emails

Let's go through these steps one by one. For validations, we define two helper methods as we want to reuse them in all editing methods:

```moonbit
///| Fails if the document is archived
fn ensure_not_archived(self : State) -> Unit!EditorError {
  guard not(self.archived) else { raise AlreadyArchived }

}

///| Fails if the given `connection_id` is not in the connection map
fn ensure_is_connected(
  self : State,
  connection_id : ConnectionId
) -> Unit!EditorError {
  guard self.connected.contains(connection_id) else {
    raise InvalidConnection(connection_id)
  }

}
```

The Unit!EditorError result type indicates that these methods can fail with EditorError.

We can also define a helper method for adding a change event to each connected editor's state:

```moonbit
///| Adds a change event to each connected editor's state
fn add_event(self : State, change : Change) -> Unit {
  for editor_state in self.connected.values() {
    editor_state.events.push(change)
  }
}
```

And finally one for resetting the email-sending deadline and list of recipients:

```moonbit
///| Updates the `email_deadline` and `email_recipients` fields after an update.
fn update_email_properties(self : State) -> Unit {
  let now = @datetime.DateTime::from_unix_mseconds(0) // TODO
  let send_at = now.inc_hour(12)
  let email_list = self.connected_editors()
  self.email_deadline = send_at
  self.email_recipients = email_list
}
```

Note that the datetime library we imported has no concept of getting the *current* date and time which we need for this function to work properly. We are going to address this problem once we start targeting WebAssembly (and Golem) as getting the current system time is something depending on the target platform.

With these helper functions, implementing the editor functions, for example add, is straightforward:

```moonbit
///| Adds a new element to the document as a connected editor
pub fn add(
  self : State,
  connection_id : ConnectionId,
  value : String
) -> Unit!EditorError {
  self.ensure_not_archived!()
  self.ensure_is_connected!(connection_id)
  self.document.add(value)
  self.add_event(Change::Added(value))
  self.update_email_properties()
}
```

Implementing poll is also easy, as we already maintain the list of changes per connection, we just need to reset it after each call:

```moonbit
///| Returns the list of changes occurred since the last call to poll
pub fn poll(
  self : State,
  connection_id : ConnectionId
) -> Array[Change]!EditorError {
  match self.connected.get(connection_id) {
    Some(editor_state) => {
      let events = editor_state.events
      editor_state.events = []
      events
    }
    None => raise InvalidConnection(connection_id)
  }
}
```

### List archiving

As mentioned in the introduction, we are going to have a singleton Golem worker to store **archived lists**. At this point we are still not having anything Golem or WebAssembly specific, like RPC calls, so let's just implement the list archive store in the simplest possible way. As I wrote earlier, we can simply store the archived lists in memory, and Golem will take care of persisting it.

We don't want to reuse the same Document type as it represents a live, editable document. Instead we define a few new types in the archive package:

```moonbit
///| Unique name of a document
type DocumentName String derive(Eq, Hash)

///| Show instance for DocumentName
impl Show for DocumentName with output(self, logger) { self._.output(logger) }

///| A single archived immutable document, encapsulating the document's name and its items
struct ArchivedDocument {
  name : DocumentName
  items : Array[String]
} derive(Show)

///| Archive is a list of archived documents
struct Archive {
  documents : Map[DocumentName, ArchivedDocument]
}
```

All we need is an insert method and a way to iterate all the archived documents:

```moonbit
///| Archives a named document
pub fn insert(
  self : Archive,
  name : DocumentName,
  items : Array[String]
) -> Unit {
  self.documents.set(name, { name, items })
}

///| Iterates all the archived documents
pub fn iter(self : Archive) -> Iter[ArchivedDocument] {
  self.documents.values()
}
```

With this done, we first implement the list archiving in the list package using simple method calls. Later we are going to replace it with Golem's own *Worker to Worker communication*.

As there will be a singleton archive worker, we can simulate this for now by having a top-level Archive instance in the archive package:

```moonbit
pub let archive: Archive = Archive::new()
```

And calling this in our State::archive method:

```moonbit
pub fn archive(self : State) -> Unit {
  self.archived = true
  let name = @archive.DocumentName("TODO")
  @archive.archive.insert(name, self.document.iter().to_array())
}
```

Note that so far we have no way to know the document's name in State - we did not store it anywhere. This is intentional, as we discussed earlier the **worker name** will be used as the document's unique identifier. Getting the worker's name will be done in a Golem specific way once we get there.

### Sending an email

We already prepared some part of the email sending logic in the State type: it has a *deadline* and a list of *recipients*. The idea is that we start an **email sending worker** when a new list is created, and this runs in parallel to our editing session, in a loop. In this loop it first queries the deadline and list of recipients from our list editing state, and then just sleeps until that given deadline. When it wakes up (after 12 hours), it queries the list again, and if it is *past* the deadline, it means there were no further editing operations in the meantime. Then it sends the notification emails to the list of recipients.

There is no library on [mooncakes](https://mooncakes.io/) yet for sending emails or even for making HTTP requests, so this is something we will have to do ourselves. Also, spawning the worker to run it in parallel is something Golem specific, so at this point we are not going to implement anything for the email package. We will get back to it once the rest of the application is already compiled as Golem components.

## Compiling as Golem Components

It is time to try to compile our code as **Golem components** - these are WebAssembly components (using the [component model](https://component-model.bytecodealliance.org/)) exporting an API described with the Wasm Interface Type (WIT) language.

### Bindings

In the current world of the WASM component model, components are defined in a spec-first way - first we write the WIT files describing types and exported interfaces, and then use a *binding generator* to generate language-specific glue code from them. Fortunately the [wit-bindgen tool](https://github.com/bytecodealliance/wit-bindgen) already has MoonBit support, so we can start by installing the latest version:

```bash
cargo install wit-bindgen-cli
```

Note that Golem's documentation recommends an older, specific version of wit-bindgen - but that version did not support MoonBit yet. The new version should work well but the example codes for Golem were not tested with it.

We will reuse the WIT definitions that were created for the Golem 1.0 launch demo.

For the list component, it is the following:

```wit
package demo:lst;

interface api {
  record connection {
    id: u64
  }

  record insert-params {
    after: string,
    value: string
  }

  variant change {
    added(string),
    deleted(string),
    inserted(insert-params)
  }

  add: func(c: connection, value: string) -> result<_, string>;
  delete: func(c: connection, value: string) -> result<_, string>;
  insert: func(c: connection, after: string, value: string) -> result<_, string>;
  get: func() -> list<string>;

  poll: func(c: connection) -> result<list<change>, string>;

  connect: func(email: string) -> tuple<connection, list<string>>;
  disconnect: func(c: connection) -> result<_, string>;
  connected-editors: func() -> list<string>;

  archive: func();
  is-archived: func() -> bool;
}

interface email-query {
  deadline: func() -> option<u64>;
  recipients: func() -> list<string>;
}

world lst  {
  // .. imports to be explained later ..

  export api;
  export email-query;
}
```

This interface definition exports two APIs - one is the public API of our list editors, very similar to the methods we already implemented for the State type. The other is an internal API for the email component to query the deadline and recipients as it was explained earlier.

For simplicity, we are using string as an error type on the public API.

For the archive component, we define a much simpler interface:

```wit
package demo:archive;

interface api {
  record archived-list {
    name: string,
    items: list<string>
  }

  store: func(name: string, items: list<string>);
  get-all: func() -> list<archived-list>;
}

world archive {
  // .. imports to be explained later ..

  export api;
}
```

And finally, for the email component:

```wit
package demo:email;

interface api {
  use golem:rpc/types@0.1.0.{uri};

  send-email: func(list-uri: uri);
}

world email {
  // .. imports to be explained later ..

  export api;
}
```

Here we are using a Golem specific type: uri. This is needed because the email workers need to call the specific list worker it was spawned from. The details of this will be explained later.

These WIT definitions need to be put in wit directories of each package, and dependencies in subdirectories of wit/deps. Check the [repository](https://github.com/vigoo/golem-moonbit-example) for reference.

We started with defining a single MoonBit **module** (identified by moon.mod.json in the root) and just created list, email and archive as internal packages. At this point we have to change this because we need to have a separate module for each chunk of code we want to compile to a separate Golem component. By running wit-bindgen in each of the three subdirectories (shown below), it actually generates module definitions for us.

We reorganize the directory structure a bit, moving src/archive to archive etc, and moving the previously written source code to archive/src. This way the generated bindings and our hand-written implementation will be put next to each other. We can also delete the top-level module definition JSON.

Now in all the three directories we can generate the bindings:

```bash
wit-bindgen moonbit wit
```

Note that once we start modifying the generated stub.wit files, running this command again will overwrite our changes. To avoid that, it can be run in the following way:

```bash
wit-bindgen moonbit wit --ignore-stub
```

With this done,

```bash
moon build --target wasm
```

will compile a WASM module for us in ./target/wasm/release/build/gen/gen.wasm. This is not yet a WASM **component** - so it's not ready to be used directly in Golem. To do so, we will have to use another command line tool, [wasm-tools](https://github.com/bytecodealliance/wasm-tools) to convert this module into a component that self-describes its higher level exported interface.

### WIT dependencies

We are going to need to depend on some WIT packages, some from WASI (WebAssembly System Interface) to access things like environment variables and the current date/time, and some Golem specific ones to implement worker-to-worker communication.

The simplest way to get the appropriate version of all the dependencies Golem provides is to use Golem's "all" packaged interfaces with the [wit-deps](https://github.com/bytecodealliance/wit-deps) tool.

So first we install wit-deps:

```bash
cargo install wit-deps-cli
```

And create a deps.toml file in each wit directory we have created with the following contents:

```yaml
all = "https://github.com/golemcloud/golem-wit/archive/main.tar.gz"
```

And finally we run the following command to fill the wit/deps directory:

```bash
wit-deps update
```

### Implementing the exports

Before setting up this compilation chain let's see how we can connect the generated bindings with our existing code. Let's start with the archive component, as it is the simplest one.

The binding generator creates a stub.mbt file at archive/gen/interface/demo/archive/api/stub.mbt with the two exported functions to be implemented. Here we face the usual question when working with code generators: we have a definition of archived-list in WIT and the binding generator generated the following MoonBit definition from it:

```moonbit
// Generated by `wit-bindgen` 0.36.0. DO NOT EDIT!

pub struct ArchivedList {
      name : String; items : Array[String]
} derive()
```

But we already defined a very similar structure called ArchivedDocument! The only differences are the use of the DocumentName newtype and that our version was deriving a Show instance. We could decide to give up using the newtype, and use the generated type in our business logic, or we could keep the generated types separated from our actual code. (This is not really specific to MoonBit or the WASM tooling, we face the same issue with any code generator based approach).

In this post I will keep the generated code separate from our already written business logic, and just show how to implement the necessary conversions to implement the stub.mbt file(s).

The first exported function to implement is called store. We can implement it by just calling insert on our singleton top level Archive as we did before when we directly wired the archive package to the list package:

```moonbit
pub fn store(name : String, items : Array[String]) -> Unit {
      @src.archive.insert(@src.DocumentName(name), items)
}
```

Note that we need to import our main archive source in the stub's package JSON:

```json
{
    "import": [
        { "path" : "demo/archive/ffi", "alias" : "ffi" },
        { "path" : "demo/archive/src", "alias" : "src" }
    ]
}
```

The second function to be implemented needs to convert between the two representations of an archived document:

```moonbit
pub fn get_all() -> Array[ArchivedList] {
  @src.archive
  .iter()
  .map(fn(archived) { { name: archived.name._, items: archived.items } })
  .to_array()
}
```

Note that for this to work, we also have to make the previously defined struct ArchivedDocument a pub struct otherwise we cannot access its name and items fields from the stub package.

(Note: at the time of writing https://github.com/bytecodealliance/wit-bindgen/pull/1100 was not merged yet, and it is needed for the binding generator to produce working code with Golem wasm-rpc; Until it is merged, it is possible to compile the fork and use it directly)

The same way we can implement the two generated stubs in the list module (in list/gen/interface/demo/lst/api/stub.mbt and list/gen/interface/demo/lst/emailQuery/stub.mbt) using our existing implementation of State.

One interesting detail is how we can map the EditorError failures into the string errors used in the WIT definition. First we define a to_string method for EditorError:

```moonbit
pub fn to_string(self : EditorError) -> String {
  match self {
    InvalidConnection(id) => "Invalid connection ID: \{id._}"
    AlreadyArchived => "Document is already archived"
  }
}
```

Then use ? and map_err in the stubs:

```moonbit
pub fn add(c : Connection, value : String) -> Result[Unit, String] {
  @src.state
  .add?(to_connection_id(c), value)
  .map_err(fn(err) { err.to_string() })
}
```

### Using host functions

When we implemented the update_email_properties function earlier, we could not properly query the current time to calculate the proper deadline. Now that we are targeting Golem, we can use the WebAssembly system interface (WASI) to access things like the system time. One way would be to use the published [wasi-bindings package](https://mooncakes.io/docs/#/yamajik/wasi-bindings/) but as we are already generating bindings from WIT anyway, we can just use our own generated bindings to imported host functions.

First, we need to import the WASI wall-clock interface into our WIT world:

```wit
world lst  {
  export api;
  export email-query;

  import wasi:clocks/wall-clock@0.2.0;
}
```

Then we regenerate the bindings (make sure to use --ignore-stub to avoid rewriting our stub implementation!) and import it into our main (src) package:

```json
{
    "import": [
        "suiyunonghen/datetime",
        { "path" : "demo/lst/interface/wasi/clocks/wallClock", "alias" : "wallClock" }
    ]
}
```

With that we can call the WASI now function to query the current system time, and convert it to the datetime module's DateTime type which we were using before:

```moonbit
///| Queries the WASI wall clock and returns it as a @datetime.DateTime
///
/// Note that DateTime has only millisecond precision
fn now() -> @datetime.DateTime {
  let wasi_now = @wallClock.now()
  let base_ms =  wasi_now.seconds.reinterpret_as_int64() * 1000;
  let nano_ms = (wasi_now.nanoseconds.reinterpret_as_int() / 1000000).to_int64();
  @datetime.DateTime::from_unix_mseconds(base_ms + nano_ms)
}
```

## Golem app manifest

In the next step of our implementation we will have to connect our two existing components: list and archive in a way that list can do remote procedure calls to archive. With the same technique we will be able to implement the third component, email which needs to be both called *from* list (when started) and called back (when getting the deadline and recipients).

Golem has tooling supporting this - but before trying to use it, let's convert our project into a **golem application** described by **app manifests**. This will enable us to use golem-cli to generate the necessary files for worker-to-worker communication, and will also make it easier to deploy the compiled components into Golem.

### The build steps

To build a single MoonBit module into a Golem component, without any worker-to-worker communication involved, we have to perform the following steps:

- (Optionally) regenerate the WIT bindings with wit-bindgen ... --ignore-stub
- Compile the MoonBit source code into a WASM module with moon build --target wasm
- Embed the WIT specification into a custom WASM section using wasm-tools component embed
- Convert the WASM module into a WASM *component* using wasm-tools component new

When we will start to use worker-to-worker communication it will require even more steps, as we are going to generate stub WIT interfaces, and compile and link multiple WASM components. An earlier version of this was [described in the Worker to Worker communication in Golem](https://blog.vigoo.dev/posts/w2w-communication-golem/) blog post last year.

The Golem app manifest and the corresponding CLI tool, introduced with **Golem 1.1**, automates all these steps for us.

### Manifest template

We start by creating a root app manifest, golem.yaml, in the root of our project. We start by setting up a temporary directory and a shared directory for the WIT dependencies we previously fetched with wit-deps:

```yaml
# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.1.0/golem.schema.json
# Schema for vscode-yaml
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.1.0/golem.schema.json

tempDir: target/golem-temp
witDeps:
 - common-wit/deps
```

By moving our previous deps.toml into common-wit and doing a wit-deps update in the root, we can fill up this deps directory with all the WASI and Golem APIs we need.

Then we define a **template** for building MoonBit components with Golem CLI. In the template, we are going to define two **profiles** - one for doing a **release** build and one for **debug**. In the post I'm only going to show the release build.

It starts by specifying some directory names and where the final WASM files will be placed:

```yaml
templates:
  moonbit:
    profiles:
      release:
        sourceWit: wit
        generatedWit: wit-generated
        componentWasm: ../target/release/{{ componentName }}.wasm
        linkedWasm: ../target/release/{{ componentName }}-linked.wasm
```

These directories are relative to the components subdirectories (for example archive) so what we say here is that once all the components are built, they all will be put in the root target/release directory.

Then we specify the **build steps**, described in the previous section:

```yaml
        build:
        - command: wit-bindgen moonbit wit-generated --ignore-stub --derive-error --derive-show
          sources:
            - wit-generated
          targets:
            - ffi
            - interface
            - world
        - command: moon build --target wasm
        - command: wasm-tools component embed wit-generated target/wasm/release/build/gen/gen.wasm -o ../target/release/{{ componentName }}.module.wasm --encoding utf16
          mkdirs:
            - ../target/release
        - command: wasm-tools component new ../target/release/{{ componentName }}.module.wasm -o ../target/release/{{ componentName }}.wasm
```

Finally, we can define additional directories to be cleaned by the golem app clean command, and we can even define custom commands to be executed with golem app xxx:

```yaml
        clean:
        - target
        - wit-generated
        customCommands:
          update-deps:
          - command: wit-deps update
            dir: ..
          regenerate-stubs:
          - command: wit-bindgen moonbit wit-generated
```

With this set, we can add a new *MoonBit module** to this **Golem project** by creating a golem.yaml in its directory - so archive/golem.yaml and list/golem.yaml for now.

In these sub-manifests we can use the above defined template to tell Golem that this is a MoonBit module. It is possible to mix Golem components written in different languages in a single application.

For example the archive component's manifest will look like this:

```yaml
# Schema for IDEA:
# $schema: https://schema.golem.cloud/app/golem/1.1.0/golem.schema.json
# Schema for vscode-yaml
# yaml-language-server: $schema=https://schema.golem.cloud/app/golem/1.1.0/golem.schema.json

components:
  archive:
    template: moonbit
```

### Building the components

With this set, the whole application (with its two already written components) can be compiled by simply saying

```bash
golem app build
```

There are a few organizational things to do first, as golem app build does some transformations on the WIT definitions. This means that our previously written **stubs** are in the wrong place. The easiest way to fix this is to delete all the wit-bindgen generated directories (but first backup the hand-written stubs!) and then copy back the stubs into the new directories created. We are not going to discuss this in more details here. The blog post incrementally discovers how to build Golem applications with MoonBit and introduces the app manifest in a late stage, but the recommended way is to start immediately with an app manifest and then there is no need to do these fixes.

### First try

Running the build command results in two WASM files that are ready to be used with Golem! Although they are not able to communicate with each other yet (so the archiving functionality does not work), it is already possible to try them out with Golem.

To do so, we can start Golem locally by downloading the latest release of [single-executable Golem](https://github.com/golemcloud/golem/releases/tag/v1.1.0) or using our hosted Golem Cloud. With the golem binary, we just use the following command to start up the services locally:

```bash
$ golem start -vv
```

Then, from the root of our project, we can upload the two compiled components using the same command:

```text
$ golem component add --component-name archive
Added new component archive

Component URN:     urn:component:bde2da89-75a8-4adf-953f-33b360c978d0
Component name:    archive
Component version: 0
Component size:    9.35 KiB
Created at:        2025-01-03 15:09:05.166785 UTC
Exports:
  demo:archive-interface/api.{get-all}() -> list<record { name: string, items: list<string> }>
  demo:archive-interface/api.{store}(name: string, items: list<string>)
and
$ golem component add --component-name list
Added new component list

Component URN:     urn:component:b6420554-62b5-4902-8994-89c692a937f7
Component name:    list
Component version: 0
Component size:    28.46 KiB
Created at:        2025-01-03 15:09:09.743733 UTC
Exports:
  demo:lst-interface/api.{add}(c: record { id: u64 }, value: string) -> result<_, string>
  demo:lst-interface/api.{archive}()
  demo:lst-interface/api.{connect}(email: string) -> tuple<record { id: u64 }, list<string>>
  demo:lst-interface/api.{connected-editors}() -> list<string>
  demo:lst-interface/api.{delete}(c: record { id: u64 }, value: string) -> result<_, string>
  demo:lst-interface/api.{disconnect}(c: record { id: u64 }) -> result<_, string>
  demo:lst-interface/api.{get}() -> list<string>
  demo:lst-interface/api.{insert}(c: record { id: u64 }, after: string, value: string) -> result<_, string>
  demo:lst-interface/api.{is-archived}() -> bool
  demo:lst-interface/api.{poll}(c: record { id: u64 }) -> result<list<variant { added(string), deleted(string), inserted(record { after: string, value: string }) }>, string>
  demo:lst-interface/email-query.{deadline}() -> option<u64>
  demo:lst-interface/email-query.{recipients}() -> list<string>
```

We can try out the archive component by first invoking the store function, and then the get-all function, using the CLI's worker invoke-and-await command:

```text
$ golem worker invoke-and-await --worker urn:worker:bde2da89-75a8-4adf-953f-33b360c978d0/archive --function 'demo:archive-interface/api.{store}' --arg '"list1"' --arg '["x", "y", "z"]'
Empty result.

$ golem worker invoke-and-await --worker urn:worker:bde2da89-75a8-4adf-953f-33b360c978d0/archive --function 'demo:archive-interface/api.{get-all}'
Invocation results in WAVE format:
- '[{name: "list1", items: ["x", "y", "z"]}]'
```

Similarly we can try out the list component, keeping in mind that the **worker name** is the list name:

When we try out list, we get an error (and if we used the debug profile - using --build-profile debug then we also get a nice call stack):

```text
Failed to create worker b6420554-62b5-4902-8994-89c692a937f7/list6: Failed to instantiate worker -1/b6420554-62b5-4902-8994-89c692a937f7/list6: error while executing at wasm backtrace:
    0: 0x19526 - wit-component:shim!indirect-wasi:clocks/wall-clock@0.2.0-now
    1: 0x414b - <unknown>!demo/lst/interface/wasi/clocks/wallClock.wasmImportNow
    2: 0x4165 - <unknown>!demo/lst/interface/wasi/clocks/wallClock.now
    3: 0x42c1 - <unknown>!demo/lst/src.now
    4: 0x433d - <unknown>!@demo/lst/src.State::update_email_properties
    5: 0x440e - <unknown>!@demo/lst/src.State::new
    6: 0x5d81 - <unknown>!*init*/38
```

The reason is we are creating a global variable of State and in its constructor we are trying to call a WASI function (to get the current date-time). This is too early for that; so let's modify the State::new method to not call any host functions:

```moonbit
///| Creates a new empty document editing state
pub fn State::new() -> State {
  let state = {
    document: Document::new(),
    connected: Map::new(),
    last_connection_id: ConnectionId(0),
    archived: false,
    email_deadline: @datetime.DateTime::from_unix_mseconds(0), // Note: can't use now() here because it will run in initialization-time (due to the global `state` variable)
    email_recipients: [],
  }
  state
}
```

This fixes the issue! Now we can create and play with our collaboratively editable lists:

```text
$ golem worker start --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list7
Added worker list7

Worker URN:    urn:worker:b6420554-62b5-4902-8994-89c692a937f7/list7
Component URN: urn:component:b6420554-62b5-4902-8994-89c692a937f7
Worker name:   list7

$ golem worker invoke-and-await --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list7 --function 'demo:lst-interface/api.{connect}' --arg '"demo@vigoo.dev"'
Invocation results in WAVE format:
- '({id: 1}, [])'

$ golem worker invoke-and-await --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list7 --function 'demo:lst-interface/api.{add}' --arg '{ id: 1}' --arg '"a"'
Invocation results in WAVE format:
- ok

$ golem worker invoke-and-await --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list7 --function 'demo:lst-interface/api.{add}' --arg '{ id: 1}' --arg '"b"'
Invocation results in WAVE format:
- ok

$ golem worker invoke-and-await --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list7 --function 'demo:lst-interface/api.{connect}' --arg '"demo2@vigoo.dev"'
Invocation results in WAVE format:
- '({id: 2}, ["a", "b"])'
```

## Worker to Worker communication

### List calling archive

The first worker-to-worker communication we want to set up is the list component calling the archive component - basically, when we call archive() on the list, it needs to call store in a singleton archive worker, sending its data to it.

The first step is to simply state this dependency in the app manifest of list:

```yaml
components:
  list:
    template: moonbit

dependencies:
  list:
  - type: wasm-rpc
    target: archive
```

Running golem app build after this will run a lot of new build steps - including generating and compiling some Rust source code, which is something that will no longer be needed in the next release of Golem.

We are not going into details of what is generated for worker to worker communication in this post - what is important is that after this change, and running build once, we can **import** a generated **stub** of our archive component in our list component's moonbit package:

```json
{
    "import": [
        "suiyunonghen/datetime",
        { "path" : "demo/lst/interface/wasi/clocks/wallClock", "alias" : "wallClock" },
        { "path" : "demo/lst/interface/demo/archive_stub/stubArchive", "alias": "stubArchive" },
        { "path" : "demo/lst/interface/golem/rpc/types", "alias": "rpcTypes" }
    ]
}
```

Then we can add the following code into our archive function to call the remote worker:

```moonbit
  let archive_component_id = "bde2da89-75a8-4adf-953f-33b360c978d0"; // TODO
  let archive = @stubArchive.Api::api({ value: "urn:worker:\{archive_component_id}/archive"});
  let name = "TODO"; // TODO

  archive.blocking_store(name, self.document.iter().to_array())
```

In line 2 we construct the remote interface by pointing to a specific **worker**, by using the component ID and the worker's name. (In the next Golem release this is going to be simplified by being able to use the component's name instead). In line 5 we call the remote store function.

What is missing are two things:

- We should not hard-code the archive component's ID as it is automatically generated when the component is first uploaded to Golem
- We need to know our own **worker name** to be used as the list's name

The solution to both is to use **environment variables** - Golem automatically sets the GOLEM_WORKER_NAME environment variable to the worker's name, and we can manually provide values to workers through custom environment variables. This allows us to inject the component ID from the outside (until a more sophisticated configuration feature is added in Golem 1.2).

We have already seen how we can use WASI to query the current date/time; we can use another WASI interface to get environment variables. So once again, we add an import to our WIT file:

```wit
  import wasi:cli/environment@0.2.0;
```

Then run golem app build to regenerate the bindings, and import it in the list/src MoonBit package:

```moonbit
        { "path" : "demo/lst/interface/wasi/cli/environment", "alias": "environment" }
```

and implement a helper function to get a specific key from the environment variables:

```moonbit
///| Gets an environment variable using WASI
fn get_env(key : String) -> String? {
  @environment.get_environment()
  .iter()
  .find_first(fn(pair) {
    pair.0 == key
  })
  .map(fn(pair) {
    pair.1
  })
}
```

We can use this to get the worker's name and the archive component ID:

```moonbit
let archive_component_id = get_env("ARCHIVE_COMPONENT_ID").or("unknown");
// ...
let name = get_env("GOLEM_WORKER_NAME").or("unknown");
```

When starting the list workers, we have to explicitly specify ARCHIVE_COMPONENT_ID:

```bash
$ golem worker start --component urn:component:b6420554-62b5-4902-8994-89c692a937f7 --worker-name list10 --env "ARCHIVE_COMPONENT_ID=bde2da89-75a8-4adf-953f-33b360c978d0"
```

With that we can try connecting to the list, adding some items and then calling archive on it, and finally calling get-all on the archive worker - we can see that the remote procedure call works!

### List and email

We haven't implemented the third component of the application yet - the one responsible for sending an email after some deadline. Setting up the component and the worker-to-worker communication works exactly the same as it was demonstrated above. The app manifest supports circular dependencies, so we can say that list depends on email via wasm-rpc, and also email depends on list via wasm-rpc. We need to communicate in both directions.

We will have to use the WASI monotonic-clock interface's subscribe-instant function to **sleep** until the given deadline.

Without showing all the details, here is the MoonBit code implementing the single send-email function we defined in the email.wit file:

```moonbit
///| Structure holding an email sender's configuration
pub(all) struct Email {
  list_worker_urn : String
}

///| Run the email sending loop
pub fn run(self : Email) -> Unit {
  while true {
    match self.get_deadline() {
      Some(epoch_ms) => {
        let now = @wallClock.now()
        let now_ms = now.seconds * 1000 +
          (now.nanoseconds.reinterpret_as_int() / 1000000).to_uint64()
        let duration_ms = epoch_ms.reinterpret_as_int64() -
          now_ms.reinterpret_as_int64()
        if duration_ms > 0 {
          sleep(duration_ms.reinterpret_as_uint64())
        } else {
          send_emails(self.get_recipients())
        }
        continue
      }
      None => break
    }
  }
}
```

We use the wall-clock interface again to query the current time and calculate the duration to sleep for based on the deadline got from the corresponding list worker. The get_deadline and get_recipients methods are just using Golem's Worker to Worker communication as shown before:

```moonbit
///| Get the current deadline from the associated list worker
fn get_deadline(self : Email) -> UInt64? {
  let api = @stubLst.EmailQuery::email_query({ value: self.list_worker_urn })
  api.blocking_deadline()
}

///| Get the current list of recipients from the associated list worker
fn get_recipients(self : Email) -> Array[String] {
  let api = @stubLst.EmailQuery::email_query({ value: self.list_worker_urn })
  api.blocking_recipients()
}
```

The two remaining interesting parts are sleeping and sending emails.

We can **sleep** by calling the subscribe-duration function in the WASI monotonic-clock package to get a pollable, and then poll for it. As we only pass a single pollable to the list, it won't return until the deadline we want to wait for expires:

```moonbit
///| Sleep for the given amount of milliseconds
fn sleep(ms : UInt64) -> Unit {
  let ns = ms * 1000000
  let pollable = @monotonicClock.subscribe_duration(ns)
  let _ = @poll.poll([pollable])
}
```

On the list side, we don't want to block until this email sending loop runs - as it would block our list from receiving new requests. The generated RPC stubs support this, we simply use the non-blocking version on the generated Api type:

```moonbit
  if not(self.email_worker_started) {
    let email_component_id = get_env("EMAIL_COMPONENT_ID").or("unknown");
    let name = get_env("GOLEM_WORKER_NAME").or("unknown")
    let self_component_id = get_env("GOLEM_COMPONENT_ID").or("unknown")
    let api = @stubEmail.Api::api({ value: "urn:worker:\{email_component_id}:\{name}"})
    api.send_email({ value: "urn:worker:\{self_component_id}:\{name}"})
    self.email_worker_started  = true;
  }
```

## Sending emails

Sending actual emails is a bit more difficult, as there are no HTTP client libraries in the MoonBit ecosystem at the moment. But Golem implements the WASI HTTP interface, so we can use the already demonstrated techniques to import WASI HTTP through WIT, generate bindings for it, and then use it from MoonBit code to send emails through a third party provider.

In the example we are going to use [Sendgrid](https://sendgrid.com/en-us) as a provider. This means we have to send a HTTP **POST** request to https://api.sendgrid.com/v3/mail/send with a pre-configured authorization header, and a JSON body describing our email sending request.

First we are going to define a few helper constants and functions to assemble the parts of the requests:

```moonbit
const AUTHORITY : String = "api.sendgrid.com"
const PATH : String = "/v3/mail/send"

type! HttpClientError String
```

The payload is a JSON, which can be constructed using MoonBit's built-in JSON literal feature. However in the WASI HTTP interface we have to write it out as a byte array. MoonBit strings are UTF-16 but SendGrid requires the payload to be in UTF-8. Unfortunately there isn't any string encoding library available for MoonBit yet, so we write a simple function that fails if any of the characters is not ASCII:

```moonbit
///| Converts a string to ASCII byte array if all characters are ASCII characters, otherwise fails
fn string_to_ascii(
  what : String,
  value : String
) -> FixedArray[Byte]!HttpClientError {
  let result = FixedArray::makei(value.length(), fn(_) { b' ' })
  for i, ch in value {
    if ch.to_int() < 256 {
      result[i] = ch.to_int().to_byte()
    } else {
      raise HttpClientError("The \{what} contains non-ASCII characters")
    }
  }
  result
}
```

With this we can construct the payload and we can also read the sendgrid API key from an environment variable:

```moonbit
///| Constructs a SendGrid send message payload as an ASCII byte array
fn payload(recipients : Array[String]) -> FixedArray[Byte]!HttpClientError {
  let email_addresses = recipients
    .iter()
    .map(fn(email) { { "email": email, "name": email } })
    .to_array()
    .to_json()
  let from : Json = { "email": "demo@vigoo.dev", "name": "Daniel Vigovszky" }
  let json : Json = {
    "personalizations": [{ "to": email_addresses, "cc": [], "bcc": [] }],
    "from": from,
    "subject": "Collaborative list editor warning",
    "content": [
      {
        "type": "text/html",
        "value": "<p>The list opened for editing has not been changed in the last 12 hours</p>",
      },
    ],
  }
  let json_str = json.to_string()
  string_to_ascii!("constructed JSON body", json_str)
}

///| Gets the SENDGRID_API_KEY environment variable as an  ASCII byte array
fn authorization_header() -> FixedArray[Byte]!HttpClientError {
  let key_str = @environment.get_environment()
    .iter()
    .find_first(fn(pair) { pair.0 == "SENDGRID_API_KEY" })
    .map(fn(pair) { pair.1 })
    .unwrap()
  string_to_ascii!(
    "provided authorization header via SENDGRID_API_KEY", key_str,
  )
}
```

The next step is to create the data structures for sending out the HTTP request. In WASI HTTP, outgoing requests are modeled as WIT **resources**, which means we have to construct them with a constructor and call various methods to set properties of the request. All these methods have a Result result type so our code is going to be quite verbose:

```moonbit
  let headers = @httpTypes.Fields::fields()
  headers
  .append("Authorization", authorization_header!())
  .map_err(fn(error) {
    HttpClientError("Failed to set Authorization header: \{error}")
  })
  .unwrap_or_error!()
  let request = @httpTypes.OutgoingRequest::outgoing_request(headers)
  request
  .set_authority(Some(AUTHORITY))
  .map_err(fn(_) { HttpClientError("Failed to set request authority") })
  .unwrap_or_error!()
  request
  .set_method(@httpTypes.Method::Post)
  .map_err(fn(_) { HttpClientError("Failed to set request method") })
  .unwrap_or_error!()
  request
  .set_path_with_query(Some(PATH))
  .map_err(fn(_) { HttpClientError("Failed to set request path") })
  .unwrap_or_error!()
  request
  .set_scheme(Some(@httpTypes.Scheme::Https))
  .map_err(fn(_) { HttpClientError("Failed to set request scheme") })
  .unwrap_or_error!()
  let outgoing_body = request
    .body()
    .map_err(fn(_) { HttpClientError("Failed to get the outgoing body") })
    .unwrap_or_error!()
  let stream = outgoing_body
    .write()
    .map_err(fn(_) {
      HttpClientError("Failed to open the outgoing body stream")
    })
    .unwrap_or_error!()
  let _ = stream
    .blocking_write_and_flush(payload!(recipients))
    .map_err(fn(error) {
      HttpClientError("Failed to write request body: \{error}")
    })
    .unwrap_or_error!()
  let _ = outgoing_body
    .finish(None)
    .map_err(fn(_) { HttpClientError("Failed to close the outgoing body") })
    .unwrap_or_error!()
```

At this point we have our request variable initialized with everything we need, so we can call the handle function to initiate the HTTP request:

```moonbit
  let future_incoming_response = @outgoingHandler.handle(request, None)
    .map_err(fn(error) { HttpClientError("Failed to send request: \{error}") })
    .unwrap_or_error!()
```

Sending a request is an async operation and what we have as a result here is just a handle for a future value we have to await somehow. As we don't want to do anything else in parallel in this example, we just write a loop that awaits for the result and checks for errors:

```moonbit
  while true {
    match future_incoming_response.get() {
      Some(Ok(Ok(response))) => {
        let status = response.status()
        if status >= 200 && status < 300 {
          break
        } else {
          raise HttpClientError("Http request returned with status \{status}")
        }
      }
      Some(Ok(Err(code))) =>
        raise HttpClientError("Http request failed with \{code}")
      Some(Err(_)) => raise HttpClientError("Http request failed")
      None => {
        let pollable = future_incoming_response.subscribe()
        let _ = @poll.poll([pollable])

      }
    }
  }
```

We are ignoring the response body in this example - but in other applications, response could be used to open an incoming body stream and read chunks from it.

With this we implemented the simplest possible way to call the SendGrid API for sending an e-mail using WASI HTTP provided by Golem.

## Debugging

When compiled to debug (using golem app build --build-profile debug), Golem shows a nice stack trace when something goes wrong in a MoonBit component. Another useful way to observe a worker is to write a **log** in it, which can be realtime watched (or queried later) using tools like golem worker connect or the Golem Console.

The best way to write logs from MoonBit is to use the WASI Logging interface. We can import it as usual in our WITs:

```wit
import wasi:logging/logging;
```

and then to our MoonBit packages:

```moonbit
        "demo/archive/interface/wasi/logging/logging"
```

and then write out log messages of various levels from our application logic:

```moonbit
let recipients = self.get_recipients();
@logging.log(@logging.Level::INFO, "", "Sending emails to recipients: \{recipients}")
match send_emails?(recipients) {
  Ok(_) => @logging.log(@logging.Level::INFO, "", "Sending emails succeeded")
  Err(error) => @logging.log(@logging.Level::ERROR, "", "Failed to send emails: \{error}")
}
```

## Conclusion

MoonBit is a nice new language that is quite powerful and expressive, and seems to be a very good fit for developing applications for Golem. The resulting WASM binaries are very small - a few tens of kilobytes for this application (only increased by the generated Rust stubs - but those are going away soon). A few things in the language felt a little bit inconvenient - but maybe it is just a matter of personal taste - mostly the JSON files describing MoonBit packages, the anonymous function syntax and the way the built-in formatter organizes things. I'm sure some of these, especially the tooling, will greatly improve in the future.

The support for WASM and the Component Model are still in an early stage - but working. It requires many manual steps, but fortunately Golem's app manifest feature can automate most of this for us. Still the generated directory structure of wit-bindgen moonbit felt a little overwhelming first.

I hope the MoonBit ecosystem will get some useful libraries in the near future, convenient wrappers for WASI and WASI HTTP, (and Golem specific ones!), string encoding utilities, etc. As there are not many libraries yet, it is very easy to find something useful to work on.

I'm looking forward to having official support for MoonBit in Golem, such as templates for the golem new ... command and extensive documentation on our website.



