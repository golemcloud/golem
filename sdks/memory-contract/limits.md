# Limits

Every Memory constructor accepts finite limits for the work of one operation. A constructor can set a
different finite value. The contract allows no unbounded setting. Exhaustion is a `limit` error that
names the limit. It is never an empty result, a truncated item or a dropped member.

The defaults are selected starting values, not measured results. Engine and SDK tasks measure them on
actual artifacts and propose changes through this contract. `fixtures/defaults-v1.json` holds the
values and their boundary cases.

| Limit | Default | Scope | Reason |
| --- | --- | --- | --- |
| Candidate rows | 1,000 | Operation, shared by composite members | `recall` and `context` return tens of items. 1,000 leaves room to interleave lexical, recency and vector candidates and to merge them before the final limit. |
| Candidate bytes | 4 MiB (4,194,304) of UTF-8 candidate text | Operation, shared by composite members | A rendered context of 128k tokens is about 0.5 MB, so packing does not run short. The value is small compared with guest memory, which the executor sets from a ratio of the host limit, not from a fixed size. |
| Vector rows | 10,000 rows scanned | Operation, per member | Bounded cosine scans over pages. 10,000 rows of 3,072 dimensions is about 31 million multiply-adds. A larger store needs an index, which the plan does not select. |
| Vector dimensions | 4,096 | Each vector | Common embedders use 1,536 or 3,072 dimensions. A larger vector is an error, not a truncation. Vectors of different models or dimensions are never compared. |
| Graph hops | 2 | Traversal, per member | The plan's initial bound. A path within the bound is returned; a longer path is not. |
| Graph visits | 1,000 nodes | Traversal, per member | Two hops from a few seeds reach hundreds of nodes. The bound stops a dense hub node from making the work unbounded. |
| Graph edges | 10,000 edges examined | Traversal, per member | About ten edges for each visited node. It bounds the work at high-degree nodes, which the visit bound alone does not. |
| Member fan-out | 8 | Composite construction, nested composites and their members included | Compositions usually combine two or three memories. Each member multiplies writes and model calls. |

Candidate bytes count UTF-8 bytes, not characters or UTF-16 code units
(`limits/candidate-bytes-count-utf8`). Temporary allocations also count toward the normal guest memory
limits.

Caller values are not defaults: the caller always supplies `limit`, `tokenBudget` and
`measureTokens`. The token budget bounds the rendered output only. Record scan, embedding and
classifier costs through normal SDK and model diagnostics.

These limits bound work. They do not bound the retained disk use of an agent; platform quotas and
retention own that.

Boundary cases: a value at the limit succeeds, and one more fails with that limit's error. The
self-check runs the candidate and fan-out boundaries on the reference memory. The vector and graph
boundaries need a backend; the backend tasks run them.
