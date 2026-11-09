# Kotlin agent memory: how it is accounted, what it costs, and where it fails

Kotlin/Wasm compiles to **WasmGC**, so a Kotlin agent's memory sits in two places: a GC heap that Golem's engine
allocates and collects, and a linear memory that carries every value crossing a host call. This report states how the
executor accounts for both, what an agent costs in practice, how it fails, and what is still unproven.

Figures were measured in November 2026 on a local debug build (16 GiB macOS host, so a 12.8 GiB admission budget) with
scratch agents that are not part of this branch. Sources are the executor's own gauges
(`golem_worker_memory_pool_used_bytes`, `worker_memory_resident_count`), `golem agent get`, k6 and the executor logs.

## Conclusions

1. **An agent is charged for what it reserves, not for what it touches.** The executor admits agents against one
   node-wide budget using reserved sizes. An idle Kotlin agent is charged about 1.4 to 1.7 MiB and uses a fraction of that.
2. **An agent is charged about 2.2 MiB.** The linear arena starts at 1 MiB and grows to the largest call, so the allocation-churn
   agent is granted **2.19 MiB** (measured at scale; two arena pages fewer since, so about 2.06 MiB). The 12.8 GiB budget holds
   about 5,900 such agents, and 5,000 fit (10.0 of 12.8 GiB).
3. **5,000 agents ran for 30 minutes with no failures.** 0 of 14,329 requests failed, there were no admission refusals, and
   both the survival and plateau gates passed. Request time at 20 requests per second offered is queueing (the executor serves
   about 8 per second); at 2 per second the median is 368 ms.
4. **A payload costs 17 to 20 times its size, and the arena is not the reason.** The arena holds each payload once (1.00 times).
   The rest is the GC heap, which reserves **exactly 2 times its peak live set**. A string needs 8 bytes per character
   (UTF-16 string and builder, doubled), and `encodeToByteArray` adds about 9 more, which the SDK no longer uses for payload strings. A `ByteArray` and a list of `Int`, `Long` or `Double` are lowered and
   lifted without an object per element and cost about 36 bytes of arena per element; every other collection costs about 900 bytes
   per element as a result and 370 as an argument.
5. **A `ByteArray` costs 36 bytes of arena per byte, a `String` 1 byte per character.** A 256 KiB `ByteArray` costs 9.6 MiB as a
   result or an argument and a `String` of the same length 6 and 4 MiB, because the host reads one schema node per element. Use a
   `String` or a stream for the largest payloads; a `ByteArray` or a `List<Int>` is fine for moderate ones.
6. **The GC heap reserves 2 times the peak live set at one moment, and garbage never raises it.** 256 MiB of garbage added nothing.
   Data accumulated gradually reserved 2.2 to 3.7 times what it holds. The limit is checked against the reservation, so with the
   default 1 GiB limit an agent can keep about 250 to 290 MiB of live data.
7. **Limit failures are loud but differently worded.** A linear-memory breach reports `Exceeded plan memory limit`. A GC-heap
   breach reports `GC heap out of memory`, not `WorkerExceededMemoryLimit`. A failed agent is recovered with
   `golem agent revert <agent> --number-of-invocations 1`, which keeps its earlier state (tried on a GC-heap failure).
8. **Tool streams are fast; HTTP streams are slow.** Tool streams move about 2 MB per second (memory not measured). HTTP
   request and response bodies move about 50 KB per second, one schema element per byte, with memory flat.
9. **A captured tool output stops at 16 MiB and the call still completes**, however much more the tool writes (tested to 64 MiB,
   30 s).
10. **Still unproven:** the GC-heap tracker's own figure (read here by difference). The arena over hours is shown in section 9.

## 1. The two regions

| Region | Holds | Typical size |
|---|---|---|
| **GC heap** (engine-owned, one per agent) | Every object, string, list and array the agent creates | Almost all of an agent's memory |
| **Linear memory** | The canonical-ABI arena that strings and lists are copied into to cross a host call | 1.06 MiB idle, plus the agent's largest call |

Wasmtime allocates the heap per store and each agent has its own store, so heaps are never shared. What agents share is the
node's admission budget. Other guests differ: a Rust agent and a MoonBit agent (as the SDK builds it, the linear-memory
`wasm` target) keep everything in linear memory. Neither SDK fixes a memory size; the Rust fixtures declare 18 to 19 pages
(about 1.2 MiB), the MoonBit SDK's tool 1 to 5, and both grow as needed. Only Kotlin splits its memory in two, so only
Kotlin is charged twice.

## 2. What the executor enforces

- **One limit across both regions** (`max_memory_per_agent`, default **1 GiB** in the registry's default plan; the local
  development server sets it unlimited). The code refuses a growth past the limit and one the node cannot admit.
- **Admission uses `max(measured resident memory, sum of reservations)` against the budget** (`worker_memory_ratio` of the
  measured limit). With many agents the sum of reservations decides, and a reservation never touched still counts.
- **A growth is admitted when it happens.** The executor evicts idle agents if it must. If there is still no headroom the
  agent traps and restarts. This does not wait.
- **A start or restart of an agent that has grown first reserves its last recorded size and waits**, retrying every 500 ms.
  That wait is the warning `Memory admission keeps refusing a worker start`. A brand new Kotlin agent declares 0 pages, so it
  does not wait.
- **Reservations are held for the agent's life.** Collection reclaims objects inside the heap but never returns capacity, and
  linear memory cannot shrink, so an agent's charge is its peak until the executor unloads it. A refused call is still
  charged: a 40,000,000 character response that the executor refused with HTTP 413 had already grown the agent by about
  650 MiB, and that stayed.
- **The component** is charged once, shared by all its agents: about 1.35 MiB for a 0.7 MB component.

## 3. What an agent costs

### Per agent

| | Value |
|---|---|
| Linear memory, idle | **1.06 MiB** (17 pages: 16 of headroom `BASE` and the arena's first page) |
| GC heap, idle | 0.25 to 0.5 MiB (granted 1.44 to 1.69 MiB in total, depending on the component) |
| Granted for the allocation-churn agent | **2.19 MiB** (about 1.0 MiB of it GC heap) |
| Executor resident memory per added agent | **0.07 to 0.7 MiB** (a bound, see below) |

**Idle heap grows with the size of the component.** An agent instance builds only the descriptor of its own type (registration is lazy
per type), yet the idle heap still rises with the number of registered types and methods (linear stays 1.06 MiB):

| Component | Registered methods | Module | Idle heap | Idle total |
|---|---|---|---|---|
| 1 type, 5 methods | 5 | 0.71 MB | 0.25 MiB | 1.31 MiB |
| 10 types, 5 methods each | 50 | 0.79 MB | 0.50 MiB | 1.56 MiB |
| 10 types x 20 | 200 | 1.00 MB | 0.50 MiB | 1.56 MiB |
| 50 types x 5 | 250 | 1.19 MB | 1.00 MiB | 2.06 MiB |
| 50 types x 20 | 1,000 | 2.23 MB | **2.00 MiB** | **3.06 MiB** |
| 1 type, 5 methods, 5,000 string literals | 5 | 0.99 MB | 0.50 MiB | 1.56 MiB |

Lazy registration halved the largest case (it was 4.00 MiB) and left the small ones where they were. What remains does not come from
descriptors, which are never built for the types an agent does not use. It comes from the classes and lambdas the module creates when it
starts, whether or not the code that uses them ever runs. A one-type, five-method component with ballast that is never called (idle heap
above the 0.25 MiB baseline; the heap is reserved in steps of 0.25, 0.5, 1, 2, 4 MiB, so each figure is the step the live set reached):

| Ballast (never run) | Module | Idle heap added |
|---|---|---|
| 8,000 / 30,000 plain functions | 1.2 / 2.8 MB | **none** |
| 5,000 / 20,000 string literals, 29 characters | 1.0 / 1.9 MB | 0.25 / 0.5 MiB |
| 20,000 string literals, 5 characters | 1.4 MB | 0.5 MiB |
| 2,000 / 8,000 classes | 1.8 / 5.3 MB | 0.75 / 1.75 MiB |
| 1,000 / 2,000 / 4,000 lambdas | 1.8 / 2.9 / 5.1 MB | 1.75 / 1.75 / 3.75 MiB |

Code size costs nothing (30,000 functions leave the heap at the baseline). A string literal costs a few bytes, whatever its length. A
class costs about 100 bytes of live heap and a lambda about 0.5 KiB, so each registered method's handler lambda is the main per-method
cost. A component with about 250 methods therefore holds 0.75 MiB, and one with about 1,000 holds 1.75 MiB, of extra idle heap in every
agent. The first agent of a component also carries the shared component charge, 2.0 times the module size (4.5 MiB for the largest above).

Resident memory cannot be pinned to one figure. Creating 5,000 agents on a clean node, the executor's own resident memory
was 339, 528, 786, 1,098, 1,338, 574, 734, 1,053 and 902 MiB at 1, 250, 500, 1,000, 1,500, 2,000, 3,000, 4,000 and 5,000
agents. It fell by 760 MiB between 1,500 and 2,000, so it does not follow the count. The granted figure was exactly 1.44 MiB
per agent throughout. The granted charge is therefore three to thirty times what an added agent actually uses.

### At scale

| | 200 agents, 5 minutes, 10 requests per second | 5,000 agents, 30 minutes, 20 requests per second offered |
|---|---|---|
| Failed requests | 0 of 3,001 | **0 of 14,329** |
| Granted per agent | 2.19 MiB | **2.19 MiB** (fitted slope) |
| Total granted | 439 MiB | 10.0 of 12.8 GiB |
| Agents resident | 200 | 4,687: every agent the load touched, never fewer |
| Executor resident memory | 425 MiB | about 0.5 to 1.5 GiB |
| Admission refusal warnings / agents waiting | 0 / 0 | 0 / at most 1 |
| Median / 95th percentile request time | 99 ms / 203 ms | 7.8 s / 10.2 s |
| Survival gate / plateau gate | | passed / passed (growth 1.2%) |

The budget holds about 5,900 agents at 2.19 MiB each.

### Request time at 5,000 agents

The 7.8 s median is queueing, not memory. The executor serves about 8 requests per second of this workload (60,000 churn
objects per call), 20 were offered, and 64 users in flight at 7.9 requests per second is 8.1 s, which is the median. At
**2 requests per second** (within capacity) the same 5,000 agents gave 1,200 requests, none failed, median 368 ms and 95th
percentile 390 ms. The first request to an agent took 369 ms against 280 ms for a repeat, so starting an agent costs about
90 ms. What limits the executor to about 8 requests per second (its CPU in a debug build, the local stack, or other load)
was not isolated.

## 4. Large values and live data

### A payload

Growth of the granted total on a fresh agent, in MiB:

| Call | Growth |
|---|---|
| Build a 1 MiB string, return only its length (GC heap only) | 8.4 |
| Return a 1 MiB string | 9.62 |
| Accept a 1 MiB string | 9.62 |
| Build an 8 MiB string, return only its length | 64.4 |
| Return an 8 MiB string | 72.62 |
| Accept an 8 MiB string | 72.62 |

- **The arena is 1.00 times the payload.** Measured from inside the call, the host writes 1,048,788 bytes for a 1,048,576 byte
  string and the SDK's lowering uses 8,388,656 bytes for 8,388,608. `golem agent get` agrees: linear memory is 1.06 MiB
  idle, 2.06 MiB after a 1 MiB result and 9.06 MiB after an 8 MiB one.
- **The rest is the GC heap, which reserves exactly 2 times the peak live set.** A string built from `n` characters reserved
  8 bytes per character plus 0.62 MiB (1 M characters 8.62 MiB, 16 M characters 128.62 MiB): 2 bytes for the UTF-16 string and 2 for
  its builder, doubled. Holding a live `encodeToByteArray()` copy as well reserved about 17 bytes per character, so that call alone
  adds about 9: it holds a temporary buffer as well as the result. The SDK therefore writes a string it lowers straight into the
  arena as UTF-8 and decodes a string it lifts straight from the arena (`Utf8.kt`), with no temporary array. A string result or
  argument costs the arena (1 byte per character) plus the 8 bytes per character a string occupies on the heap: 9.62 MiB for
  1 MiB of text and 72.62 MiB for 8 MiB, in either direction. Malformed UTF-8 on the way in falls back to `decodeToString`, so
  it is replaced exactly as before.
- **Collections cost per element, not per byte.** Granted growth in MiB (arena in brackets):

| Collection | n | Build only | As result | As argument |
|---|---|---|---|---|
| `List<Int>` | 10,000 / 100,000 | 1.5 / 10.0 | 1.81 / 13.4 (3.4) | 0.31 / 4.25 (3.4) |
| `List<String>` | 10,000 / 100,000 | 1.5 / 20.5 | 3.9 / 24.9 (4.4) | 3.9 / 24.9 (4.4) |
| `List<Record>` of 3 fields | 10,000 / 50,000 | 3.5 / 15.5 | 5.0 / 22.9 (7.4) | 5.0 / 22.9 (7.4) |

  A list of `Int`, `Long`, `Double` or `String` (and a `ByteArray`) is held as primitives or strings while it crosses the boundary, so
  it costs the arena's 32 bytes per node plus 4 for the index (35 bytes per `Int` element), the elements themselves and the agent's own
  list: about 140 bytes per `Int` element as a result and 45 as an argument at 100,000 elements, and about 260 bytes per `String`
  element in either direction. A record costs about 480 bytes in either direction at 50,000 records: the arena's nodes, the agent's own list and nothing per node, because elements are converted one at a time (a list lowered from a result converts each element twice, to count and to write; a list lifted from an argument reads the arena as the agent consumes it). The arena part is the schema tree's 32 bytes per node, set by the wire format. The tree is written straight into
  the arena in two passes (count the nodes, then write them), with no node object, closure or `Pair` per node. A list lifted from an argument is valid until the call ends and fails loudly if read afterwards. (`List<Long>` and
  `List<Double>` take the same path as `List<Int>` and are tested against the generic lowering, not measured end to end.)
- **A 64 MiB string failed the agent** (building it needs a heap past the default 1 GiB limit). A 16,000,000 byte result left
  the agent holding 212 MiB.
- **`ByteArray` against `String`**, 256 KiB payload:

| | As a result | As an argument | Built inside the agent |
|---|---|---|---|
| `String` | 6.0 MiB | 4.0 MiB | 2.6 MiB |
| `ByteArray` | 9.63 MiB | 9.63 MiB | 0.56 MiB |

A `ByteArray` crosses the boundary as a `list<u8>`: one schema node per byte (32 bytes of arena) and a 4-byte index, 9 MiB for 256 KiB,
plus the 0.6 MiB of the agent's own array and call. No object is created per byte. A `String` costs 1 byte per character in the arena, so
it is the cheaper carrier by about 1.6 to 2.4 times; inside the agent a `ByteArray` is the cheaper type.

### Live data

The reservation follows the **peak live set at one moment**, at 2 times for one allocation:

| What | Granted growth |
|---|---|
| 64 and 256 MiB of pure garbage in 1 KiB objects | **+0.00** (garbage never raises it) |
| One array of 8 / 16 / 32 / 64 / 128 MiB | 16.06 / 32.06 / 64.06 / 128.06 / 256.06 MiB (exactly 2 times) |
| 16 / 64 MiB held and then dropped | 40.5 / 163.5 MiB (the peak stays) |
| Byte arrays accumulated 20 to 100 MiB at a time | 75, 150, 298, 594, 1,187, 2,371 MiB at 20, 40 to 60, 100, 200, 300 to 500, 600 to 1,100 MiB held |

Data accumulated gradually reserved in steps about twice the previous one (37 MiB times a power of two), 2.2 to 3.7 times what
was held. Either way, every byte removed from what is live at the same moment saves two bytes of charge; allocation count and
garbage do not matter. The limit is checked against the **next** reservation, so with the default 1 GiB an agent can keep about
250 to 290 MiB: the 594 MiB step fits and the 1,187 MiB step does not. Size memory limits against an agent's **peak** payloads
and live data, and expect a few hundred MiB for an agent that handles payloads up to the executor's limits (8 MiB requests,
16 MiB responses).

## 5. Streams

| Stream | Result | Speed | Memory |
|---|---|---|---|
| HTTP response, producer: 1 MiB / 8 MiB | complete (21 s / 159 s) | about 50 KB/s | flat |
| HTTP response, producer: 32 MiB | stopped by the client's 300 s limit at 15.7 MB | about 50 KB/s | one heap step |
| HTTP request body read: 1 MiB / 4 MiB | received exactly (8 s / 22 s) | about 50 KB/s | flat |
| Tool stdin to a Kotlin tool, captured stdout: 1, 4, 16 MiB | complete (1, 2, 8 s) | about 2 MB/s | not measured |
| Same, 17 to 64 MiB | all processed; capture stops at 16 MiB with `ResourceExhausted` (9 to 30 s: 17 MiB 9 s, 32 MiB 18 s, 48 MiB 24 s, 64 MiB 30 s) | about 2 MB/s | not measured |
| Same, 64 MiB, no capture | complete (6 s) | about 11 MB/s | not measured |

HTTP bodies are streams of `list<u8>` and each byte is one schema element, which `docs/api/http-routers.md` documents as not
tuned for multi-megabyte payloads. Tool streams are read and written in 64 KiB chunks released after each, with explicit
16 MiB ceilings on `readAll` and on captured stdout and stderr.

**Captured output ceiling.** A captured stdout or stderr stops at 16 MiB and reports `ResourceExhausted`; the call still completes,
however much more the tool writes, because the caller releases the output's readable end as soon as it stops
(`ToolAttachments.drive`, as the middleware path does). An executor test checks this with a 48 MiB
stream. A Node test cannot, because it needs host imports.

## 6. When an agent passes its limit

Driving one agent past three limits gave the same behaviour:

| Limit | Last call that worked | Failed call |
|---|---|---|
| 256 MiB | 60 MiB held (reservation 150 MiB) | needed a 300 MiB heap |
| 1 GiB (default) | 200 MiB held (reservation 594 MiB) | needed a 1,187 MiB heap |
| 4 GiB | 1,100 MiB held (reservation 2,371 MiB) | needed a 4,742 MiB heap |

| | GC heap | Linear memory |
|---|---|---|
| Caller sees | `INTERNAL_AGENT_EXECUTION_FAILED`, `GC heap out of memory: no capacity for allocation of N bytes` | `Component trapped: Exceeded plan memory limit` |
| Executor logs | neither `WorkerExceededMemoryLimit` nor `WorkerOutOfMemory` appears | |
| Afterwards | reservation released at once; later calls fail with `Previous invocation failed`; `golem agent get` shows `Failed`, retry count 2; another agent on the node was unaffected | later calls fail with `Previous invocation failed: Exceeded plan memory limit`; the reservation was released |

**Recovery.** `golem agent resume` fails again. **`golem agent revert <agent> --number-of-invocations 1 --yes` works**: it removes
the failed invocation and the agent answers again with its earlier state (a 60 MiB hold came back, reservation 150.6 MiB).
`golem agent delete` also works but starts a fresh agent with no state.

## 7. The linear arena

`cloud/golem/wasm/Memory.kt` is the bump allocator behind every canonical-ABI buffer. It sits above a 1 MiB `BASE`, starts
with an 8-byte canary, and extends linear memory only as far as an allocation reaches, in whole pages. It is reset after every
call, so its size is set by the largest single call, and the pages stay because linear memory cannot shrink. An agent is
charged `BASE` plus its largest call.

- **Headroom.** Kotlin's own scoped allocator starts at address 0 and resets after each scope, so the headroom (1 MiB by default,
  `BASE` below) is the room it gets below the arena (the arena starts exactly at the headroom, and the canary is the last 8 bytes
  of it). A stdlib scope larger than that writes into the arena (printing a 2 MiB string does). The arena
  cannot move and a larger `BASE` would be charged to every agent, so the canary is checked on every allocation and a clobbered
  arena fails the next allocation with "a stdlib scoped allocation larger than the headroom ... overwrote it". Keep strings
  given to `print` and `println` well under 1 MiB. Detection comes at the next allocation, so what the scope itself wrote over
  is already lost.
- **The headroom is a per-component setting.** `wasmComponent { arenaHeadroomKiB.set(n) }` in the agent's build (whole 64 KiB pages,
  64 to 16384) becomes the KSP option `golem.arenaHeadroomKiB`; the generated entry point then calls `configureArenaHeadroom` at
  module start, in `cabi_realloc` and at the top of every registration function, before the arena exists. Unset, the generated code
  is unchanged. A different size after the arena exists is an error, and setting it without the KSP plugin fails the build. Measured
  idle linear memory of an agent that makes a trivial stdlib call (a clock read):

| Headroom | Idle linear memory | Saved against the default |
|---|---|---|
| 64 KiB | 0.25 MiB | 0.94 MiB |
| 128 KiB | 0.375 MiB | 0.81 MiB |
| 256 KiB | 0.50 MiB | 0.69 MiB |
| 1024 KiB (default) | 1.19 MiB | |

  (An agent that makes no stdlib call is two pages smaller, 1.06 MiB at the default: the stdlib grows memory by two pages the first time
  it uses its allocator.)
- **A print close to the headroom still works; one over it fails the agent.** In real agents at 128 KiB, a `println` of 60, 90, 110
  and 125 KiB succeeded and 140 KiB trapped; at 256 KiB, 100 and 200 KiB succeeded and 400 KiB trapped; at the default 1 MiB, 400 KiB
  succeeded. A failed agent reports `Status: Failed` and answers `Previous invocation failed` until it is reverted or deleted. The
  margin is only a few KiB, so the headroom should exceed the longest string the agent prints by a comfortable amount.
- **The limit** is the end of the 32-bit address space, after which the agent's memory limit and the node's capacity apply.
- **Growth mid-call works.** Calls that grew the arena part-way through, up to 16 MB, succeeded with no warnings.
- **The headroom a stdlib scope needs is the length of what it prints.** With guard words at 8 to 512 KiB, a `println` of 10, 100,
  200 and 400 KiB overwrote guards up to about its own size; `println` of 1 KiB, stack traces at depth 50 and 500, `Random` and the
  clock stayed under 8 KiB.
- **Memory is grown to the pages the arena reaches, no more.** The stdlib's scoped allocator grows memory only when a request of `n`
  bytes has `floor(n / 64 KiB) + 1` pages above the current size, and then to `floor(n / 64 KiB) + 2` pages (1,500 KiB gave 25 pages,
  2,048 KiB 34, 3,000 KiB 48, 5,000 KiB 80). The arena asks for two pages fewer, or one more when only one page is missing (so a
  growth can overshoot by one page), and initialisation claims the arena's first page up front: an idle agent is 17 pages. A read
  of the last byte of every growth traps at once, on the first growth of any agent, if a stdlib ever grows differently.

## 8. Accepted trade-offs

GC-heap bytes are **not metered**, so they do not reach the byte-time accounting and write no oplog growth hint; only linear
memory is. That is deliberate.

- **Billing undercounts Kotlin.** An agent holding 500 MB of objects can cost less than a Rust agent holding 500 MB in linear
  memory. Making GC bytes billable is a pricing decision, not a runtime one.
- **GC-heap objects count toward the limit.** An agent that grows past it fails. If a Kotlin agent fails with
  `GC heap out of memory`, raise its limit before assuming a leak.
- **Space is not returned early.** A brief spike holds part of the limit and the node's capacity until the agent ends.
- **One combined limit, provisionally.** It is simpler for users; a limit per region would allow GC headroom without widening the
  linear allowance. Which is right is open.

The accounting lives in `golem-worker-executor`: `services/gc_heap.rs` (the tracker), `DurableWorkerCtx::admit_gc_heap_growth` and
`admit_unshared_memory_growth` (admission), and `services/active_agents` (the gate).

## 9. Verification

| Layer | What it proves | Result |
|---|---|---|
| Tracker bookkeeping, `services/gc_heap.rs` `tests` (6) | A completed growth keeps its reservation, a failed one releases it, several merge into one, the size never falls. Inert reservations, so bookkeeping only | passed |
| Real host capacity, `tests::host_capacity` (3) | Headroom falls by exactly the heap size on commit and returns to baseline on drop or failure; merged growths lose nothing. This is the leak check | passed |
| The cap, `durable_host/mod.rs` `gc_heap_cap_tests` (6) | At the limit is admitted, past it refused, linear memory held reduces the heap's headroom (the limit binds the two regions together), an overflow is refused | passed |
| Packed lists, `PackedListTest.kt` (8) | A `ByteArray` and lists of `Int`, `Long` and `Double` lower to exactly the tree a generic list does (compared node by node, at lengths from 0 to 70,001), round-trip through the arena, still reject an element of the wrong kind, and the helpers still accept an unpacked list | passed |
| The arena, `MemoryTest.kt` (17) | Invalid requests fail, the first allocation starts at the headroom, a configured headroom moves the arena and the canary and a different one after use fails, out-of-range headrooms are refused, a 40 MiB block is usable, a small agent grows by at most a page, resets keep the capacity, a 2 MiB `println` is detected and a short one is not | passed |
| String transcoding, `Utf8Test.kt` (9) | UTF-8 written to and read from the arena equals `encodeToByteArray` and `decodeToString` for ASCII, 2-, 3- and 4-byte text, surrogate pairs, lone surrogates, malformed bytes, 1,900 random cases and 1 MiB of text | passed |
| SDK node suite | | **320 passed, 0 failed** |
| KSP tests, including `ArenaHeadroomTest` (4) | The option is absent from the generated code unless set, is emitted at module start, in `cabi_realloc` and in each registration function, accepts 64 and 16384 KiB and rejects anything else naming the option | **213 passed** |
| Gradle plugin tests, including `ArenaHeadroomTest` (6) | `arenaHeadroomKiB` is passed to KSP as `golem.arenaHeadroomKiB` only when set, bad values fail naming the setting, and setting it without the KSP plugin fails the build | **21 passed** |
| Executor `gc_heap` tests | | **15 passed, 0 failed** |
| Contract suite, 56 probes | Real agents start, deploy, serve HTTP, stream, snapshot and run tools with the accounting live | **56 passed** |
| Arena headroom check (a second small app with `arenaHeadroomKiB` = 256) | The arena starts at the headroom, a print of three quarters of it works and leaves the agent usable, and an agent is 384 KiB; fails two of four when the setting is not applied | **PASS=4 FAIL=0** |
| Tool middleware executor check | Tool middleware and streams on a real executor, including a 48 MiB stream | **all checks passed** (27 `ok` lines) |

Run the executor tests with `cargo test -p golem-worker-executor --lib -- gc_heap --report-time`, and the arena tests with
`./gradlew wasmWasiNodeTest --rerun-tasks` in `sdks/kotlin/sdk`. Three traps: the headroom of the fake probe in the capacity tests
assumes zero resident usage (give it a non-zero value above the amount granted and headroom stops tracking reservations); a
Node test file that reaches a host import fails the whole run, and a compile error leaves the previous run's XML in place, so read
the exit code and file times, not only the totals; `resetArenaForTest()` clears the bookkeeping but not the memory.

### The arena over 8 hours

5,000 agents, 5 requests per second, 8 hours, the debug executor with a local Postgres and Redis stack, the SDK as built with lazy lists
and lazy registration. All 144,001 requests succeeded and the executor logged no `WorkerOutOfMemory` and no admission refusal. Both
gates passed: survival, and resident memory (growth 2.4% between the third and fourth quarters, limit 15%). The executor's resident
memory settled at about 500 MiB after the first hour and stayed there, with spikes to about 1,500 MiB. Response times: median 286 ms,
90th percentile 1.86 s, 95th percentile 2.2 s, maximum 4.9 s. k6 recorded no timings over time, so the lower panel of the chart
shows requests in flight divided by the arrival rate (Little's law), the mean time in the system. Granted memory per agent was not
measured in this run; the 2.19 MiB figure is from the 30 minute run.

## 10. Still unproven

- **The tracker's own figure.** The GC reservation is the pool gauge minus the linear memory `golem agent get` prints, both read
  directly. The tracker is not exported as a gauge, and its debug log never appears because `golem server run` applies its own
  tracing directives after `RUST_LOG`.
- **The remaining SDK-side reductions.** Direct codecs for lists of strings and records and for the rest of the type system, and lazy
  descriptors, are measured and ranked in `.claude/plans/kotlin-agent-memory-efficiency.md`; string transcoding, the arena's pages, the
  headroom setting and the packed `ByteArray` and primitive lists are done.
- **The at-scale figures are from before the last arena change.** The 200-agent and 5,000-agent runs were made with an idle linear
  memory of 1.19 MiB; it is now 1.06 MiB (about 0.125 MiB less per agent), measured on one agent, not at scale.
- **What limits the executor to about 8 requests per second.**
