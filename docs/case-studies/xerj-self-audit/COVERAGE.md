# Coverage — what the substrate sees, and what it can't

Honest accounting of the audit's reach. The value claim depends on this being
truthful, so the limits are stated as plainly as the coverage.

## What is covered (measured)

| | count |
|---|---|
| Rust files under `engine/` (excl. `target/`) | 197 |
| Files parsed with **0 tree-sitter ERROR nodes** | 197 (100%) |
| `function_item` AST nodes | 5,095 |
| Function records emitted | 5,095 (**100.0%**) |
| Call-graph edges | 87,137 |
| Route registrations | 477 |
| axum-handler-shaped functions | 299 |
| `unsafe` blocks (non-test) | 22 |

Function coverage is asserted, not assumed: `coverage.json` records both the
independent count of `function_item` nodes and the number of emitted records, and
they must be equal. The first extractor cut reported 97.2% — nested `fn` items
inside function bodies were being skipped. That was a real extractor bug; it is
fixed (the walker descends into function bodies) and the two counts now match.

Per-file coverage, error-node counts, and the full unsafe inventory are in
`coverage.json` / `unsafe_inventory.json`.

## What the AST substrate cannot see (stated limits)

The extractor is a **lexical + structural** analysis. It gives functions their
scope and the call graph their edges; it does **not** do type inference or
dataflow. Concretely, out of reach:

- **Trait / dynamic dispatch.** A `Box<dyn Trait>` call resolves at runtime; the
  call graph cannot say which impl runs. Edges through dynamic dispatch are
  missing.
- **Method-call edges are deliberately excluded.** A method call (`x.len()`)
  cannot be resolved to a definition without knowing the receiver's type.
  Resolving by bare name wires every same-named method together — measured, that
  collapsed 603 functions into one spurious strongly-connected component and hid
  the real recursion cycle. The call graph therefore contains **free and
  path-qualified calls only** (`foo()`, `Type::foo()`, `module::foo()`). This is
  why the recursion-cycle finder works (parser functions are free functions) but
  would miss a cycle that passes through a trait method.
- **Macro-generated code.** Bodies inside declarative/procedural macros are not
  expanded; `#[derive(...)]` and route-registration macros are seen only as text.
- **`build.rs` / codegen.** Generated sources are not in the tree at audit time.
- **Callee name collisions across crates.** Where a free-function name is not
  unique, resolution falls back same-file → same-crate → global; the cycle finder
  reports its ambiguous-resolution count so the graph's approximation is visible
  (≈4k ambiguous of 87k edges).
- **Signal patterns are substrings.** `sinks`, `validators`, `unsafe_ops` are
  matched as substrings against function text. They are a *triage* signal to
  narrow the read set — every finding is confirmed by **reading the real code**,
  never by the pattern alone. Bodies over 12,000 chars are truncated in the index
  (39 functions; flagged `body_truncated:true`) so the auditor knows to open the
  file.

## What this means for the findings

- The **recursion-cycle** result (F1 recall + the SQL Critical) is strong: those
  are free-function cycles the graph represents exactly, and the SQL finding was
  additionally proven by execution.
- The **unsafe inventory** is complete for `unsafe` that appears in source text
  (100% of files parsed); it does not cover unsafety introduced by macro
  expansion.
- Lenses that need dataflow (true taint from an extractor param to a sink across
  method calls) are **approximated** by co-occurrence within a function plus the
  free-function call graph — good enough to triage, and every candidate is read
  before it is believed. A finding that would require proving a value is
  attacker-controlled *through* trait dispatch is out of this substrate's reach
  and is not claimed.
