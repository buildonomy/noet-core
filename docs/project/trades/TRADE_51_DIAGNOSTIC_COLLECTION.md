# Trade Study: Diagnostic Collection for Author-Facing Errors

**Issue**: 51 — Author Feedback: Compiler Diagnostics Pipeline
**Status**: Decision Pending
**Authors**: Andrew Lyjak, Claude Sonnet 4.6
**Date**: 2025-10-28

## Problem Statement

`check_for_link_and_push` (and several other codec paths in `md.rs`) currently call
`tracing::info!` / `tracing::warn!` when an author-visible error occurs (unresolved link,
malformed frontmatter). That information is invisible to callers. We need it in
`ParseResult.diagnostics` so authors get structured, actionable feedback from `noet build`.

Two architecturally distinct approaches exist. This study evaluates them.

---

## Option A: Out-Parameter Threading

Thread `diagnostics: &mut Vec<ParseDiagnostic>` through the call chain from codec internals
back to `GraphBuilder::parse_content`, which already owns a `diagnostics` vec.

### Scope of Change

- `check_for_link_and_push` (private fn) gains one parameter
- `MdCodec::inject_context` collects diagnostics from that call and appends to a local vec
- `GraphBuilder::parse_content` passes `&mut diagnostics` into `inject_context` (requires
  adding the parameter to the `DocCodec` trait, or collecting within `MdCodec` and flushing
  through a new trait method)
- `parse_sections_metadata` and `finalize` follow the same pattern for their warn calls

The `DocCodec` trait is the pinch point. If the parameter is added to `inject_context` and
`finalize`, every implementor must be updated (currently: `MdCodec`, `NetworkCodec`, and any
wrapper codecs). That is mechanical but compiler-enforced — the compiler will catch every
missed site.

### Data Flow

```
check_for_link_and_push(…, diagnostics: &mut Vec<…>)
  └─ push UnresolvedReference / Warning
MdCodec::inject_context(…, diagnostics: &mut Vec<…>)
  └─ forward to check_for_link_and_push, collect result
GraphBuilder::parse_content
  └─ pass &mut local_diagnostics to inject_context
  └─ local_diagnostics flushed into ParseContentResult
DocumentCompiler::parse_next
  └─ ParseContentResult.diagnostics → ParseResult.diagnostics
```

### Pros

- **Type-safe end to end.** Diagnostics are strongly typed `ParseDiagnostic` values from
  emission site to CLI output. No intermediate serialization/deserialization.
- **No global state.** No subscriber registration, no `Arc<Mutex<…>>`, no thread-local.
  Data flows through the ordinary call stack.
- **Location information is first class.** `UnresolvedReference.reference_location` is
  populated at the point where `link_data.range` is available — no round-trip through
  tracing field encoding to recover it.
- **Synchronous and deterministic.** Diagnostics arrive in call order, associated with the
  exact document being parsed. No risk of interleaving from concurrent parses.
- **Testable in isolation.** Pass a `Vec` into the function and assert on its contents.
  No subscriber setup required.
- **No new dependencies.**

### Cons

- **`DocCodec` trait signature change** if the parameter is added there. All current and
  future implementors must handle it. A default empty-vec pattern can reduce boilerplate,
  but the change is still broad.
- **Does not capture diagnostics from code that does not accept the parameter.** Any
  tracing call inside a library dependency or deep utility not on the threading path stays
  invisible. For the current scope this is not a problem; it could become one if the
  diagnostic surface grows significantly.
- **Requires conscious plumbing at every new call site.** Future codec authors must remember
  to thread the vec.

---

## Option B: Tracing Subscriber Buffer

Implement a `tracing_subscriber::Layer` (modeled on the existing `CaptureStateRouterLayer`
from the instrumentation crate) that captures events with a sentinel header into an
in-memory buffer. The compiler activates the buffer before parsing a document and drains
it into `ParseResult.diagnostics` afterward.

### Scope of Change

- New `DiagnosticCaptureLayer` that holds `Arc<Mutex<Vec<CapturedEvent>>>`.
- Emit diagnostics via existing `tracing::warn!` calls, adding a structured field:
  `header = "NOET_DIAGNOSTIC:<severity>"` alongside the existing human-readable message.
- Compiler calls `layer.start_capture()` before `parse_content`, then `layer.drain()` after.
- Drain converts `CapturedEvent` → `ParseDiagnostic::Warning` / `ParseDiagnostic::Info`.
- No `DocCodec` trait change required.

### Data Flow

```
tracing::warn!(header = "NOET_DIAGNOSTIC:warning", doc_path, attempted_keys = …, …)
  └─ DiagnosticCaptureLayer::on_event
       └─ push CapturedEvent into Arc<Mutex<Vec<…>>>

DocumentCompiler::parse_next
  └─ layer.start_capture()
  └─ parse_content(…)   ← all tracing calls inside captured
  └─ events = layer.drain()
  └─ parse_result.diagnostics.extend(events.into_parse_diagnostics())
```

### Pros

- **No `DocCodec` trait change.** Existing codec code needs only its `tracing::warn!` calls
  updated to emit the sentinel header field. Call signatures are untouched.
- **Captures from any depth.** Any code on the call path — including library code that also
  uses `tracing` — can emit capturable events without knowing about `ParseDiagnostic`.
- **Compatible with the existing instrumentation architecture.** The
  `CaptureStateRouterLayer` pattern is already understood in this codebase and the
  `tracing-subscriber` dependency is already present (`Cargo.toml` confirms this).
- **Gradual adoption.** Existing `tracing::warn!` calls can be converted one at a time by
  adding the header field; no sweeping refactor needed.

### Cons

- **Global subscriber required.** The layer must be registered in a global
  `tracing::subscriber`. In tests that do not initialize a subscriber this silently drops
  events. In library code (`noet-core` is a library crate) forcing a global subscriber is
  an intrusive contract with callers — if the caller has already set a subscriber, adding
  another layer requires composing them at init time.
- **Concurrent parse hazard.** `DocumentCompiler` currently processes one document at a
  time through `parse_next`, but nothing prevents concurrent callers. A single global
  buffer captures events from all concurrent parses indiscriminately. Scoping requires
  either thread-local storage or passing a session token through every tracing callsite —
  which reintroduces the threading problem.
- **Location information is degraded.** Byte ranges from `link_data.range` cannot be
  encoded in tracing fields without custom serialization. Line/col numbers must be computed
  before the `tracing::warn!` call and passed as integer fields, which is less ergonomic
  and requires the source string to be in scope anyway.
- **Type fidelity is lost at emission.** `attempted_keys: Vec<NodeKey>` becomes a
  `Debug`-formatted string in the tracing field. The drain must re-parse or accept lossy
  strings. `ParseDiagnostic` ends up carrying `Warning(String)` rather than a structured
  type — which is acceptable today but complicates LSP integration in Issue 11, where
  `tower-lsp` expects structured `Diagnostic` objects with specific fields.
- **`start_capture` / `drain` are stateful and fragile.** If `parse_content` panics or
  returns early, the drain must still be called or stale events leak into the next parse.
  Requires a guard type (like `std::panic::catch_unwind` wrapper or RAII drain).
- **New dependency on subscriber infrastructure in core compile path.** The instrumentation
  crate uses `uniffi` and is designed for mobile FFI. Pulling it into `noet-core`'s compile
  path as-is would add heavyweight dependencies. A purpose-built buffer layer avoids that,
  but is more code.

---

## Comparison Matrix

| Criterion                              | Option A (Out-Parameter) | Option B (Subscriber Buffer) |
|----------------------------------------|--------------------------|------------------------------|
| Type safety of captured data           | ✅ Full                  | ⚠️  Lossy (string fields)    |
| Location precision (byte→line/col)     | ✅ Native                | ⚠️  Requires pre-computation |
| Concurrent parse safety                | ✅ Inherent              | ❌ Requires extra design      |
| No global subscriber required          | ✅                       | ❌                            |
| DocCodec trait signature change        | ❌ Required              | ✅ Not required               |
| Works in library crate without init    | ✅                       | ❌                            |
| LSP Issue 11 compatibility             | ✅ Structured types      | ⚠️  Needs re-structuring      |
| Testable without subscriber setup      | ✅                       | ❌                            |
| Captures diagnostics from any depth    | ⚠️  Only threaded paths  | ✅                            |
| New dependencies                       | ✅ None                  | ⚠️  Subscriber infrastructure |

---

## Hybrid Consideration

A hybrid is worth naming but not recommending here: use the subscriber buffer only for
tracing calls in code that *cannot* be threaded (third-party or deeply nested), and use
out-parameters everywhere under direct control. This adds the complexity of both approaches
without fully eliminating the cons of either. Reject for now.

---

## Recommendation: Option A

The cons of Option B are structural, not incidental:

1. **The concurrent-parse hazard is a correctness risk**, not a performance concern.
   `noet-core` is a library; callers may parallelize `parse_next` at any time. A global
   buffer with session tokens to scope captures would re-introduce parameter threading at
   the subscriber level, defeating the stated benefit.

2. **Type fidelity loss is a direct cost to Issue 11.** The LSP server needs
   `attempted_keys: Vec<NodeKey>` as structured data to build `lsp_types::Diagnostic`
   objects. Recovering that from a `Debug`-formatted string is fragile and wrong.

3. **The `DocCodec` trait change, while broad, is bounded and compiler-enforced.** The
   number of implementors is small and known. The change is mechanical. This is the normal
   cost of extending a trait and is preferable to correctness and type-safety regressions.

The subscriber architecture from the instrumentation crate is well-suited to its original
purpose: capturing high-frequency, loosely-structured sensor data from deeply nested mobile
code where call-signature changes are impractical. Document parse diagnostics are
low-frequency, require strong typing, and flow through code we own and can modify. Option A
is the right tool for this problem.

---

## Decision

- **Chosen**: Option A — Out-Parameter Threading
- **Revisit if**: concurrent parsing is introduced (multiple documents parsed in parallel),
  at which point a per-task diagnostics handle (e.g., `tokio::task_local!`) would be a
  cleaner solution than either approach above.