# Cognitive complexity

Scores every Rust function by cognitive complexity (SonarSource's measure: branches and loops
cost more the deeper they nest) and fails if any scores above 10. There's no allowlist; simplify
the function instead.

```sh
cargo build --release --manifest-path tools/cognitive-complexity/Cargo.toml
tools/cognitive-complexity/target/release/cognitive-complexity           # every tracked Rust file
tools/cognitive-complexity/target/release/cognitive-complexity <file.rs> # only these files
```

Run it from the repository root. It skips vendored code under `patches/` and prints each failing
function as `score  path:line  name`, worst first. It parses source with `syn`, so it needs no
build and takes about a second. Macro bodies count only when they parse as expressions or
statements (`assert!`, `vec!`, `format!`).

The scoring rules are at the top of `src/score.rs`; the limit is `LIMIT` in `src/main.rs`. CI
runs it whenever a Rust file changes.
