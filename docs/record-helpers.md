# Record helpers

Each TypeScript helper of `CostTracker` records one call's usage.
`recordCaptcha` records one solve and `recordSms` one message, each at the
pricing table's per-unit price. `recordEmail(provider, count?, override?)`
records `count` emails at the provider's per-email price, one email when the
caller gives no count. `recordProxyBytes`, `recordLlm` and `recordCompute`
record the bytes, tokens and seconds they are given. `recordLlm` (Python
`record_llm`) files every model call under the service `llm` with the model as
the resource: the price catalogue declares no provider for a model, so none is
guessed from words in its name. `recordCompute` (Python `record_compute`) files
every instance under the service `compute` with the instance type as the
resource, for the same reason: an undeclared instance type names no provider.
A model call is priced at the catalogue entry whose key is the model name
(case aside) or at the catalogue's `default` entry, never at a listed model
whose key its name contains; the record's `metadata.priced_as` names that key
(`llmPriceKey`, Python `llm_price_key`), so a default price reads as one. A
call recorded with an override carries no `priced_as`.

## The agent a record is attributed to

The TypeScript `record` takes a `RecordInput`, a `CostRecord` without `agent_id`, and sets the
tracker's own `agent_id` on the record it buffers and returns, so the record a
caller gets back, `snapshot().records` and the rows a sink receives all carry
it. `RecordInput` omits `agent_id` from fresh object literals, but TypeScript's
structural typing can still accept an existing `CostRecord` variable. Every
record naming another agent, including those ordinary typed values and
JavaScript inputs, is refused at runtime before anything is buffered:

```
TypeError: CostTracker.record: agent_id "<given>" is not this tracker's agent "<own>"; record another agent's spend with a CostTracker for that agent
```

Record that spend with a tracker created for that agent. A record naming the
tracker's own agent is kept. On tracker-created records, `agent_id` is
enumerable but cannot be reassigned, deleted or redefined, including through
the record returned by `record` or exposed by `snapshot().records`. Assignment
throws a `TypeError` in strict code, including ES modules. `CostRecord` declares
`agent_id` readonly; other fields remain mutable for edits before `flush`,
and sinks receive their current values,
not an immutable snapshot. Change those fields before calling `flush`; changing
a record shared with `MemorySink` also changes its in-memory read-back.

`record` keeps a shallow copy of the caller's `metadata` object (nested values
remain shared), or an empty object when it is omitted. Later top-level changes
to the caller's object do not alter this copy. A supplied value must be a plain
object, with `Object.prototype` or a null prototype; null, arrays, strings,
dates and maps are refused before buffering:

```
TypeError: CostTracker.record: metadata must be a plain object when supplied; omit it when no metadata is available
```

These refusal and ownership rules are specific to the TypeScript tracker;
the Python tracker sets its own agent when it creates the record and accepts
no caller-supplied agent. Git-pinned
consumers receive these changes only when they update their dependency
revision. Before updating, remove foreign-agent inputs, avoid assigning the
returned `agent_id`, and supply plain-object metadata.

`created_at` is set by `record` when the caller leaves it out. A record written
to a sink directly keeps it absent in `FileSink` and `MemorySink`, and the
Supabase table fills it with its column default `now()`.

## Exit hooks

Unless `autoFlush: false`, the tracker attempts to flush pending records on
`beforeExit`, SIGINT and SIGTERM, reporting failed writes on stderr.
On natural exit, a failed flush reports the error and suspends that tracker's
`beforeExit` listener if no new record arrived during the attempt. Accepting
a new record re-arms the listener; the next natural exit attempts all pending
records, including the failed batch. Without new input, automatic shutdown
does not repeat the same failed write. Pending records remain available to an
explicit `flush`.
Without an application listener, each tracker removes its own listener after
the flush attempt. The last tracker to finish raises the signal again, so
the process ends by that signal as it would without a tracker.
Trackers from independently loaded copies of the package coordinate through
the same listener marker, rather than treating each other as applications.

An application listener is invoked only by the original signal: the tracker
does not raise it again while that listener remains, and keeps its own listener
installed so records added before a later signal are flushed too. Repeated
signals during a write share the current flush; they do not cancel a stuck
sink. Ownership is captured when the signal arrives, before awaiting the sink:
an application `once` listener keeps that signal's shutdown decision even
after Node removes its registration while invoking it.

The application keeps the shutdown decision. A listener that exits must await
every tracker's `flush` first; otherwise its exit can interrupt a write.
A listener that does not exit leaves the process running. Call `flush`
explicitly when the caller must handle a failed write.

## Run the attribution journeys

From the repository root:

```sh
node tests/spend/js/attribution-run.mjs
```

The runner builds the actual npm package and exercises record attribution,
metadata refusals, service totals, persisted files and process termination.
Signal cases include application-owned shutdown, records between signals,
failed file writes and repeated signals while a real FIFO-backed read blocks.
It also packs that build, installs the archive in the run's isolated npm
directory and runs `tests/spend/js/local.mjs` against that installation,
checking persisted charges, agent attribution, overlapping flushes, pricing
and refusals without substituting another package from the workspace.
It requires Node, npm, git, a tracked git checkout and `mkfifo` on a POSIX host.
This runner qualifies staged inputs: stage the implementation, tests, manifests,
README and this page before running it. An unstaged input is refused rather than
qualifying a different tree. To exercise working changes without this
qualification check, use `npm run build && node --test tests/spend/js/attribution.test.mjs`.

Each run prints a report path under `.build/attribution/`. The report retains
source revision, staged-tree identity, dirty state, diff, source and
built-artifact hashes, runtime identity, commands, exit statuses and stdout/stderr.
Every hashed input, including this page and README, must be tracked and match
the index before execution. A changed source during execution or a failed
command fails the run; earlier successful reports do not qualify a later
source tree. Optional arguments are passed to Node's test runner.
