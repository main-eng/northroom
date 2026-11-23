---
name: code-standards
description: Northroom's coding standards for its Rust (core, GPUI shell, crates) and its small TypeScript build scripts. Names that say what a function does, one name per concept, no magic values, parse at the boundary and trust the types, one owner per default, exhaustive matches, no boolean flag parameters, tests through seams, named errors, no hidden global state, an injected clock. Use it whenever writing, editing, refactoring or reviewing code in this repo, even if standards aren't mentioned.
---

# Northroom code standards

`DECISIONS.md` and `CONTEXT.md` win when they disagree with this file. When Clippy fires, fix
the code; don't allow the lint.

## 1. Names

- Name a function for what it does, verb then object (`resolve_release`, `read_catalog`), not
  how (`process`, `handle_data`).
- One concept, one name, everywhere, in code and in copy.
- Put the unit in the name of a number (`timeout_ms`, `duration_frames`).

## 2. No magic values

Type the domain first (an enum, a newtype), then name the values. A meaningful string or number
gets one `const` in the module that owns it.

## 3. Parse at the boundary, trust the types inside

Parse input from people, disk, the network or another program once, where it arrives, into a
type that can only hold valid values. After that, no defensive fallbacks: no
`unwrap_or_default()` on a value the types guarantee, no re-validation. Real absence is
`Option`; "absent" versus "cleared" is `Option<Option<T>>`.

## 4. One owner per default

Set a default where the value is made (parsing, a constructor), never again at each reader.

## 5. Search before writing a helper

Look for an existing function in the owning module first. Two helpers that do the same thing
will drift.

## 6. Make illegal states unrepresentable

Prefer an enum to flags that must agree. Match every variant; no catch-all `_` arm on a domain
enum, so a new variant breaks the build where it matters.

## 7. No boolean flag parameters

`render(page, true)` says nothing at the call site. Split the function or take an enum.

## 8. Errors are named and seen

- One error type per module (`thiserror`), with variants that say what failed.
- A failure is returned, retried on purpose, or logged with enough to act on. Never swallowed.

## 9. No hidden state

No module-level mutable state. State lives in a value that's passed in. Time comes from an
injected clock, so tests can hold it still.

## 10. Tests go through seams

Replace a dependency through a seam the code already has (a trait such as `Clock`, a fixture
drive), never by patching a module. A bug fix comes with a test that fails without it.

## 11. Build scripts

The few TypeScript scripts (the syntax highlighter bundle, the sample files) follow the same
rules: no new dependency unless writing it ourselves is clearly worse.

## 12. Small functions

Every Rust function stays at cognitive complexity 10 or below (`tools/cognitive-complexity`, run
in CI). Get there with steps a reader would name, early returns, `let … else` and matching on
tuples. Not with pass-through helpers, `part_one` splits or logic hidden in macros.

## 13. Plan for the next version

Before shipping anything that outlives a release (a file in someone's folder, output an agent or
script reads, a setting), say how a later version will change it, in the ADR or the pull request.
If there's no good answer, change the design.

- Keep knowledge in the app, not in copies. A copy written into someone's folder or another
  program's config is frozen at the version that wrote it.
- One reader per format. When a format changes, remove the old reader in the same change
  (ADR-0011). Settings fall back field by field. Never rewrite someone's files unless they ask.
- What agents and scripts read only grows. Commands, flags and JSON gain fields and never drop or
  rename one silently. A removed command fails with a message naming its replacement.
- The app's own data can go. Preferences and caches parse field by field; data the app stops
  using is deleted by the version that stops using it.

## Reviewing

Check a diff against the sections above and the owning docs. Report each finding with the file
and line, the rule, and the smallest fix. Don't restyle unrelated code.
