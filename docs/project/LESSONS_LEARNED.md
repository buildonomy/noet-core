# Lessons Learned

Durable engineering lessons extracted from noet-core's development. Each entry
records a **pattern that transfers** — a failure mode, a diagnostic technique,
or a design constraint that cost real investigation time and would otherwise be
rediscovered.

**What belongs here**: mechanisms, not events. "A CPU-bound task with no yield
points starves a single-threaded runtime, and the symptom looks like lock
contention" is a lesson. "Fixed the parse slowdown in commit abc123" is not.

**What does not belong here**: per-run measurements (see the performance log),
current work status (see `docs/project/0_open/`), or API documentation (see
`docs/design/`).

**Why this file exists**: much of this knowledge currently lives only in commit
messages. Those messages are unusually detailed, but they are not discoverable
— nobody greps `git log` for a bug they have not yet had. This file is also the
migration target for that content when the history is rewritten
(see the public-release audit).

---

## Diagnosis

### A symptom in one subsystem often has its cause in another

Three separate investigations mistook the display for the owner:

- **sqlx "slow acquire" warnings that were not lock contention.** A
  single-threaded runtime cannot poll a task that is ready; the acquire
  timer measures wall-clock from request to completion, which includes
  time sitting in the ready queue. The fix was the runtime, not the pool.
- **A Phase 4 balance panic that was not about heading levels.** A document
  whose first heading skipped levels (`###` before `#`) panicked, so the
  heading gap looked causal. It was not: the gap merely produced enough
  structural depth for an unrelated PathMap deletion bug to reach nodes
  that were not direct children of the network root. The gap made the bug
  *visible*, not present.
- **An "asset not found" crash at the last step of a build.** The BID was
  correctly in the global store and correctly absent from the session
  store; the crash site was several phases downstream of the sync function
  that should have copied it.

**Practice**: identify which subsystem *owns* a problem versus which one
*displays* it. Trace data flow backwards from the symptom before changing
anything at the symptom site.

### Measure before modelling, and check the model's assumptions

An analytically reasonable cost model (`n²/4` insert shift for a wide
container) was empirically inapplicable: sort keys are assigned
monotonically per sink, so inserts tail-append and measured shift was
*exactly zero*. The model assumed uniformly random insert positions, a
distribution the system never produces.

**Practice**: state a model's distributional assumption explicitly, then
check that the system produces that distribution before optimising against
it.

### An at-rest measurement cannot rule out a transient state

A data structure carried a `Vec` to hold a rare duplicate. An end-of-run scan
found zero duplicates on a large corpus, and the obvious conclusion was that
the `Vec` could collapse to a single value. Instrumenting the *live* path
instead showed 0.145% of lookups returning multiple candidates, with a maximum
of 34. Both measurements were correct: none survived to the end, and dozens
co-existed while parsing. Acting on the first would have converted a rare live
state into silent data loss.

Then the same trap in reverse. The transient state was real, but it was not the
mechanism the `Vec` existed for. The tolerated case was "a content node claims
a URL a stub already holds", which can produce at most two claimants — it could
never explain 34. Examined properly, the duplicates were two unrelated defects
compounding: a config option over-applied 51,591 registrations where ~1,111
were intended, and a path-construction bug then flattened all of them onto the
same malformed string. Fixing both took multi-candidate lookups to zero across
1.29M calls on the same corpus. The structure had been defending against its
own bugs, and the mid-run measurement was quantifying them.

**Practice**: when a structure exists to tolerate a transient state, measure it
*during* the transient, not after — steady-state counts answer "does this
persist", never "does this occur". Then check that what you measured is the
state the structure was built for. A count that exceeds what the intended
mechanism can produce (here: 34 claimants for a mechanism capped at two) is
evidence of a *different* cause, not a more severe version of the expected one.

### A large metric is not necessarily a binding constraint

Removing 422M string comparisons per corpus run — a 99.98% reduction in
scan work — changed wall clock by about 1%. A sequential `Vec` scan is
memory-bandwidth-cheap; the cost was real but not on the critical path.

**Practice**: establish that a cost is on the critical path before
optimising it. Report both the metric and the wall-clock effect, especially
when they disagree; a headline improvement of four orders of magnitude with
no user-visible change is a result worth stating plainly.

### Silence is where the time goes

On a mature build, the largest single unexplained cost was a contiguous
block with *no log output at all*. Gaps between consecutive log lines have
repeatedly been more informative than any instrumented counter, because
instrumentation only measures what someone already suspected.

**Practice**: on any unexplained wall-clock complaint, measure inter-line
gaps first. Instrument second.

### A probe that fires zero times is evidence, if you can prove it could fire

Four hypotheses about the source of a malformed path were killed by probes that
never emitted a line: two in the parent-resolution walk, one in the
anchor-normalisation carve-out, and one earlier at a different call site. Each
zero was worth more than a positive hit would have been — it eliminated a
whole region of the pipeline in one corpus run rather than narrowing within it.

The discipline that makes a zero meaningful is proving the probe *can* fire.
The first zero was ambiguous, because a probe compiled out, mis-targeted, or
placed on a branch that is never taken is indistinguishable from a probe
reporting a true absence. Confirming reachability took two forms: a unit test
that drove the guarded branch and observed the emission, and mutation —
inverting the condition to check the probe then fires. Only after that does
"zero hits on a corpus with 51,591 known instances" mean the mechanism is
somewhere else.

The corollary is that a disproven hypothesis should be recorded where the next
reader will look. Each ruled-out site kept a short comment naming what was
tested and that it fired zero times, so the same four theories are not
re-investigated. Deleting the probe and leaving no trace loses the finding.

**Practice**: before believing a silent probe, demonstrate it can speak — by
test or by mutation. Then record the negative result at the site it exonerates,
not only in the commit message.

### Inherited configuration needs a scope, not just a value

A `alias-template` declared on one network was inherited by every document
beneath it and applied to every node in each — depth × per-node, with no way to
narrow either axis. That is correct for hand-authored sections carrying
meaningful external keys, and catastrophic for machine-generated ones: an
importer that turns each slide of a deck into a heading with a positional id
produced 51,591 registrations where ~1,111 were intended, none of them the keys
the template was written for.

The failure is not that the default was wrong — the original consumers need it.
It is that a setting which propagates across an unbounded subtree shipped
without a way to say *which* nodes it reaches, so the only available answer was
"all of them".

Note also which axis mattered. A depth limit would not have helped: the
documents needing the alias and the documents needing none were siblings in the
same directory. Only a per-node opt-in distinguished them.

**Practice**: when config propagates by inheritance, ship the scope control
with the value, defaulting to the existing behaviour. Ask what happens when a
subtree contains generated content, since generated content is where
per-node cost multiplies without anyone choosing it.

### A function that silently does nothing is worse than one that fails

A sync function existed specifically to close a known gap between two
stores. It logged "Merged 0 nodes" on every invocation since introduction
and was never observed to merge anything — its query used a traversal
direction that could not reach the nodes it was meant to copy. The bug
survived because a no-op looks identical to "nothing to do".

**Practice**: when a function's whole purpose is to move data, assert or
warn when it moves none. Zero is a suspicious result for an operation that
was called deliberately.

---

## Performance patterns

### Hub nodes turn innocuous traversals into corpus-wide fan-out

Several distinct pathologies traced to one structural fact: a namespace
root accumulating tens of thousands of direct children.

- A 1-hop neighbour query from any leaf pulled the entire corpus.
- A linear subtree scan never terminated early, because every entry
  trivially matched an empty order prefix (`starts_with(&[])`).
- Per-file work scaled with total accumulated namespace size rather than
  with the file.

**Practice**: watch node degree, not just node count. Any traversal whose
cost is proportional to a neighbour set needs a bound on how large that
set can grow, or an explicit exclusion for known hubs.

### Rebuild-on-read is O(n²) hiding in plain sight

Two independent hot spots had the same shape: an index rebuilt from scratch
after every mutation, or recomputed on every read accessor. Both looked
like simple bookkeeping and both dominated the profile. The fixes were
incremental maintenance at each mutation site, and (for a graph) a data
structure whose indices survive removal.

**Practice**: an index that is cheap to rebuild once is not cheap to
rebuild N times. Check whether "rebuild" sits inside a loop or an accessor.

### A bounded algorithm can be bracketed by unbounded setup

A subgraph extractor ran a correctly DFS-bounded traversal — and wrapped it
in two full-graph passes: an index over every node, built to resolve a
single seed, and a scan over every edge, filtered afterwards to the
reachable set. Called once per network, this was O(networks × graph size).
It accounted for 95% of per-task setup on a large corpus; removing it cut
the parse phase from 48 to 21 minutes.

The bounded core is what draws the eye during review, and it was correct.
The cost was entirely in the two lines before and after it.

**Practice**: when auditing a hot function, cost the setup and teardown
separately from the algorithm. Ask what each line's cost is proportional to
— if any is proportional to the *whole* collection while the function's
purpose is to work on a *part*, that is the bug, regardless of how the
middle is written.

### One defect of a class implies others; audit rather than celebrate

After fixing the above, an audit for the same shape — "work proportional to
the whole collection, executed once per item" — immediately found a second
instance in the same module: a path-collision check linear-scanning the
entire map on every generated path, from a per-edge call site, examining
1.68M entries where the corpus had ~2.8k states. The index needed to fix it
had been added earlier by a different change that converted only one of the
two call sites.

The audit also found four more instances that were measured and
*deliberately left alone* (7.1% of one constructor), which is the other half
of the practice: an audit that fixes everything it finds is not an audit,
it is scope creep.

**Practice**: when a performance defect has a nameable shape, grep for the
shape before closing. Record what you measured and chose not to fix, so the
next audit starts from evidence instead of rediscovery.

### Estimate cost from what a line is proportional to, not from how it reads

Several of these defects survived review because the expensive constructs
look cheap in isolation: `g.node_indices().map(...).collect()`,
`existing_map.iter().any(...)`, `values_mut().for_each(...)`. Each is one
idiomatic line. What makes them quadratic is the *call site*, which is
usually in a different function and often a different file.

The reverse error also occurred: an analytically alarming `n²/4` insert
shift measured at exactly zero, because the input distribution was
monotonic (see "Measure before modelling").

**Practice**: for any collection operation in a function you are reviewing,
ask two questions — what is this proportional to, and how many times is
this function called? A cost is a product of the two, and neither is
visible from the line itself. Then measure, because both answers are
frequently wrong.

### Bracket a suspected hot spot before optimising inside it

Attribution beat intuition repeatedly in one investigation. The standing
hypothesis was that a graph *union* dominated per-task setup; timing the
three sub-steps separately showed union at 2.9% and an index *rebuild* at
95.5%. Profiling one level further attributed 99.3% of that rebuild to a
single callee, and one level further still to a single function within it.
Each level redirected the fix; acting on any earlier level would have
optimised the wrong thing.

A proposed invasive redesign (sharing immutable state across tasks) was
abandoned as a result: the cheap local fix removed the cost that justified
it.

**Practice**: when a span is expensive, time its sub-steps before changing
any of them, and emit input sizes alongside durations so cost-per-unit can
be checked for superlinearity. Descend one level at a time — the first
plausible culprit is frequently not the last.

### Count the unit of work, not the number of calls

A lookup had two routes: an indexed one and an exhaustive fallback for keys
missing from the index. The fallback was **20% of calls** — easy to dismiss —
but **97.7% of the actual work**, because each fallback call probed all 1,131
networks while an indexed call probed ~7. It cost 175s per build and resolved
nothing: every one of its 66.6M probes returned `None`, which the index could
have answered for free.

Three prior attempts narrowed the *indexed* route on the reasoning that its
candidate set looked too wide. They moved wall clock by roughly nothing, +2%,
and 1.25x. A counter separating calls from probes found the real 84.6% on its
first run, and deleting the fallback gave 5.1x.

One of those attempts made things *worse* — 86% worse — while backed by a
confident quantitative argument. The arithmetic was sound; it was calibrated
on a corpus snapshot that no longer matched the one being measured.

**Practice**: when a code path has multiple routes, instrument the unit of work
(probes, comparisons, bytes) separately per route, not just the call count. The
expensive route is often the rarer one, which is exactly what call-count
intuition gets wrong. And before optimising a lookup's *search*, check whether
a miss is already provable without searching.

### Deduplicate work at the call site, not inside the callee

A hot loop issued one expensive query per relation. Checking whether the
target was already present before querying reduced call volume by ~6x on a
representative corpus; the per-call cost never changed.

**Practice**: before optimising an expensive operation, count how many
times it is invoked redundantly.

### Instrumentation must be free when disabled

`tracing` defers *field expressions* until after the level check, so
expensive values in a `debug!` are safe. It does **not** defer an enclosing
`if`. A probe guarded by `if const_namespaces().contains(sink)` allocated
and compared on every call — in the hot path — to serve a probe that fired
109 times.

Similarly, an O(n) invariant check called from an insert routine makes bulk
insertion O(n²). Correctness checks belong at batch boundaries, behind an
env var, not per mutation.

**Practice**: put cheap values inside the macro; gate expensive checks at
batch boundaries and behind an explicit opt-in.

---

## Design constraints

### The graph may cycle; every construction over it must be resilient

Authors write across repositories, and no build-time check can stop them from
creating a cycle. Per-kind acyclicity is therefore *reported*, not enforced:
`built_in_test` collects SCC violations into an error list and the graph is
built anyway. Refusing to build would be a worse failure than reporting one.

The obligation lands on the consumer. **Every traversal must have a named
termination argument**, and there are only three legitimate ones:

| Mechanism | Use when | Example |
|---|---|---|
| **Visited set** | completeness is required | reachability walks; closure hashes; the query evaluator's frontier retain |
| **Depth cap** | the bound is semantic, and cost falls on a shared resource | `MAX_TRAVERSAL` in query evaluation |
| **Documented cutoff** | neither fits, and the number has a rationale | `BALANCE_CUTOFF`, deliberately 15 rather than 10 |

A depth cap is the weakest of the three and is often reached for first. It does
not bound *work*: one hop from a hub node can touch more of a corpus than ten
hops down a chain. A visited set bounds both work and termination, which is why
it is the default for anything that must be complete.

The subtle failure is a construction that guards one path and not another. A
path-building routine detected back edges during initial construction and
skipped them, but its incremental update path performed no such check — so the
same relation behaved differently depending on whether it arrived during the
first build or as a later event, and only the second path could not terminate.

**Practice**: when adding a traversal, name which of the three mechanisms it
uses and say so in a comment. When a structure has both a batch-construction
path and an incremental path, verify the guard exists on *both* — a test that
builds the same graph each way and compares results is the cheapest oracle.
Never assume an invariant that is only checked, never enforced.

### A forward-compatibility field is a liability unless something exercises it

"It costs nothing to carry" is false. A field written but never read is not
free — it is unverified data accumulating, and on the day something finally
depends on it, it is as likely to be garbage as useful. The failure is silent:
the field looks populated, nothing rejects it, and whatever it encodes is wrong.

Logical clocks are the canonical case. A clock is only correct if *every* writer
maintains it correctly, and a single-writer deployment never exercises that
property.

**Practice**: any field justified by forward compatibility must name the
mechanism that exercises it *now*. If none exists, omit the field. If the field
is genuinely unretrofittable — an ordering cannot be reconstructed for records
written before it existed — then carry it, but make something read it: route a
real code path through the field so ordinary use verifies it, and assert the
property in a test. "Exercised by a test that would fail if it were wrong" is
the bar; "stored correctly" is not.

The corollary is a triage question worth asking of any speculative field: is it
**omittable** (add it later at no cost), **unretrofittable** (must be carried
from the first record), or **inert** (carried but unread)? Only the first two are
legitimate; the third is the case that rots.

### A document states what is; the journey belongs to the history

A revision note inlined into a document — "an earlier version said X, that was
wrong because Y" — reads like diligence and is a defect. It puts two claims in
one place with no marker of which is load-bearing, and every downstream reader
and every cross-reference now has to disambiguate them. **The enemy of context is
noise**, and a superseded claim adjacent to a current one is the highest-density
noise available: it is on-topic, plausible, and false.

The journey has its own homes, and they are the ones built for it: **commit
messages** (what changed and why, per change), **issues** (what the problem
state is, why it is a problem, what to do), **prompts and scratchpads**
(ephemeral working context), and **this file** (patterns that transfer). Version
control exists to make the path traversable; duplicating it into the artifact
defeats the separation.

The exception is narrow and identifiable: a document whose *subject* is a state
of affairs — an issue, a problem report, a decision log in a planning tracker.
There the history is the content. In a design document, a specification, a
procedure, or a reference, it is not.

**Practice**: when a document is corrected, write the corrected claim and delete
the wrong one. Put the reason in the commit message. Two supersession patterns
are legitimate in the document itself, and both are pointers rather than
narration:

- **A supersession pointer** at the top of a section: "superseded by X; read
  that instead." One sentence, no argument, no retained prose.
- **A method note** stating what is true about how to arrive at the content —
  "this set is reached by traversal, not search" — which is durable guidance,
  not a record of a past mistake.

The tell that the line has been crossed: the words *earlier*, *previously*,
*originally*, *the first draft*, *was wrong*, *has been corrected*. In a document
that is not an issue or a log, each of these is a candidate for deletion.

## Identity and caching

### An identity that must survive a rebuild must be derived, not minted

A minted identity draws on the clock or on entropy, so the same inputs produce a
different value on the next run. A derived identity is a pure function of its
inputs, so it does not. Minting is right for anything whose identity is
established once and then persisted; it is wrong for anything reconstructed from
source on every build.

The failure is invisible in the test that would catch it. Within one process the
minted value is held in memory and every reference resolves, so the object looks
stable. It destabilizes only across a process boundary — a rebuild, a cold CI
run, a regenerated corpus — which is exactly the boundary a unit test does not
cross. The same defect was found three times in unrelated subsystems: content
nodes re-minting when no prior store resolved them, record identities colliding
across concurrent writers of one actor, and a folded record projecting to a
freshly-minted graph node on every fold.

**Practice**: for each durable identity, name the function that produces it and
the inputs that function reads. If any input is the clock, entropy, or
iteration order, then either (a) a persistence path is part of the design and
must be tested across a restart, or (b) the identity is wrong. "It is stable
because nothing re-runs it" is not a persistence path. Test identity stability
across a process boundary and a serialization round trip, never by hashing the
same in-memory value twice.

### Do not conflate identities that have different scopes

A single `id` field served as both a document-scoped HTML anchor and a
network-scoped unique identifier. When collision resolution assigned a
system-generated value for the network scope, it corrupted the anchor,
which changed the stored path, which caused cache misses on re-parse,
which created duplicate nodes on every run. One overloaded field produced a
four-step failure chain.

**Practice**: when one field answers two questions with different scopes,
split it before the scopes diverge. An enum that names each state is
cheaper than tracing the chain later.

### The source file must win over cached derived state

A cached identifier, stale by one pass, overwrote the value parsed from the
source file. Because the pipeline then wrote its own output back to disk,
the divergence produced an infinite rewrite loop: each pass disagreed with
the file it had just written.

**Practice**: in a pipeline that rewrites its own inputs, define
unambiguously which side wins on conflict, and prefer the source. Test for
convergence explicitly — parse twice and assert the second pass is a no-op.

### A derived `Clone` can share what the type appears to own

`PathMapMap` holds `BTreeMap<Bref, Arc<RwLock<PathMap>>>` and derives `Clone`.
That clone copies `Arc` *handles*, not `PathMap`s — so every clone silently
aliased the original's path maps, and a write through one was visible through
all of them. The type read as owning its contents; `#[derive(Clone)]` gave it
reference semantics instead. The aliasing existed unnoticed for a long time
because the surviving clones happened not to write to the same networks.

The fix is copy-on-write at the shared unit: clone the entry when
`Arc::strong_count > 1`, immediately before taking a write guard. That turned
the latent bug into the mechanism that let epoch tasks share a prebuilt index
instead of each rebuilding it.

**Practice**: when a struct holds `Arc<Mutex<_>>`/`Arc<RwLock<_>>` and derives
`Clone`, decide explicitly whether clones should share or copy, and write it
in the doc comment. "Whether two clones can observe each other's writes" is
part of a type's contract, not an implementation detail. A `RwLock` makes such
a write *safe*, not *private* — the compiler will never flag the difference.

### Cheap identity checks fail on incidental sharing

Content-addressed identity churns when content changes; path-addressed
identity is stable under edits. A hash-derived node ID was reverted for
exactly this reason — content edits changed the ID and forced downstream
rewrites — but the reversal went further than necessary, to a
non-reproducible time-based ID. The distinction worth preserving: a
*key*-derived deterministic ID is stable and computable; a *content*-derived
one is neither.

**Practice**: derive identity from the thing that names an entity, not from
the thing that describes it.

---

## Concurrency

### A single-threaded runtime cannot execute concurrent readers

Splitting a mutex into a read-write lock was necessary but not sufficient:
under a current-thread runtime, permitting concurrent readers changes
nothing, because there is only one thread to run them on. CPU-bound work
with no `await` points never yields, so one task starves all others.

**Practice**: check the runtime flavour before concluding a locking change
will help. Concurrency permitted is not concurrency executed.

### A test that awaits a timer cannot catch a test that blocks on CPU

The regression test for the locking change above passed under both
runtimes, because its mock awaited `sleep` — a real yield point — while
production code blocked on computation. The test could not fail for the
production failure mode.

**Practice**: when testing scheduling behaviour, the mock must block the
same way production does. A test that cannot fail for the mechanism it
guards is worse than no test, because it produces false confidence.

### Parallel and sequential paths diverge in what state they carry

Per-task workers construct fresh state; a long-lived sequential builder
accumulates it. Bugs that depend on accumulated state appear in one mode
and not the other, and vice versa for bugs that depend on state *not*
being carried across tasks.

**Practice**: when a bug reproduces under `--jobs 1` but not `--jobs N` (or
the reverse), suspect state lifetime before suspecting a race.

### Redundant *rebuilding* and redundant *content* are separate costs

Parallel tasks each rebuilt an index over a snapshot that was 99.99% identical
across tasks. Building it once per epoch and cloning it cut seeding ~4.8x. But
every task still *carries* the whole snapshot: sharing removed the cost of
repeatedly deriving the data, not of holding it. The remedy for the second
cost (fetch only what a task references) is a different change with different
risk, and the first does not approximate it.

A related trap in the same area: a proposal to shrink the seed by *reshaping*
the namespace into a hierarchy would not have shrunk it at all, because the
seeding traversal walks to every leaf either way — re-parenting leaves under
intermediates leaves the reachable set the same size, or slightly larger.

**Practice**: when the same data is derived N times, separate "how often is
this computed" from "how much of it is needed." Sharing addresses the first;
only demand-driven fetching addresses the second. State which one a change
claims to fix.

---

## Working with logs

### Strip ANSI before grepping

The tracing subscriber colourises output even when stderr is redirected to
a file, wrapping span names, field names, and separators individually. A
prefix that reads `parse_task{task_idx=0 …}` on screen is
`^[[1mparse_task^[[0m^[[1m{^[[0m…` on disk, so `grep -c 'parse_task{'`
returns **0** on a log containing 12,046 of them.

This produced one wrong conclusion (that spans were not rendered at all)
that survived until someone checked the raw bytes.

**Practice**: `sed 's/\x1b\[[0-9;]*m//g'` before any hand-check. Analysis
tooling should strip internally.

### Do not extrapolate from a partial run

A mid-run projection from a checkpoint missed the final figure by ~2.5x,
because the phases that dominate the tail had not started yet.

**Practice**: quote totals from completed runs only.

### Confirm which flags the harness actually sets

Two investigations nearly compared incomparable runs: one used a
persistent on-disk cache, the other an ephemeral in-memory one, because
the build recipe never passed the flag that enables persistence. A
regression visible in one is structurally invisible to the other.

**Practice**: before comparing two measurements, verify they exercised the
same code path.

### Verify the exit code, not the output

Test output can look clean while the run failed. Always capture combined
stdout/stderr, check the exit code explicitly, and grep for failures before
drawing conclusions.

---

## Tooling and environment

### Sandboxed agents cannot receive filesystem notifications

File-watcher tests fail 100% inside a sandboxed terminal and pass 100%
outside it, because the sandbox blocks OS file-change notifications. This
looks exactly like flakiness and wasted two separate investigations,
including one that concluded "pre-existing flake" on the basis of runs that
were themselves all sandboxed or all unsandboxed.

**Practice**: run watch and notification tests unsandboxed. When a test
fails only in one environment, suspect the environment before the test.

### A ratchet over an enumerated list cannot see an unenumerated leak

The neutrality audit greps for a denylist of customer, program, and system
names. A design doc's worked example used a source path and type name copied
verbatim out of a real corpus — deployment-derived, and exactly what the rule
forbids — and the audit passed both before and after, because the path contained
no listed term. The check was working as designed; the design cannot cover this.

The same blindness applies to anything structural rather than nominal: a
directory layout, a dependency list, a set of symbol names. Each is derived from
a real deployment and none trips a name-based search.

**Practice**: treat a green audit as "no known name recurred", not "this is
neutral". Invent examples rather than pasting them — a made-up
`src/widget/include/widget/Widget.h` teaches the same mechanic as a real path
and cannot leak. The reflex to copy a working example from the corpus you are
debugging is the specific thing to resist.

### Duplicated maintenance sites are how invariants rot

Three separate code paths rebuilt the same set of indices with copy-pasted
loops. Adding a fourth index meant finding all three — and the failure mode
for missing one is not a crash but a silently unresolved lookup.

**Practice**: when the same invariant is restored in more than one place,
extract the restoration into one function before adding to it.

---

## See also

- `docs/design/core/beliefbase_architecture.md` — the specifications these
  lessons constrain
- `docs/project/0_open/` — current work
- `AGENTS.md` § "Known Pitfalls" — the subset of these that agents hit most
  often, kept there for discoverability during code work
