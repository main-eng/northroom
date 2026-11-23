//! Fails when a Rust function's cognitive complexity is above `LIMIT`.
//!
//! `cognitive-complexity` scores every Rust file git tracks under the current directory except
//! vendored `patches/`; `cognitive-complexity <file>…` scores just those files. Each function
//! above the limit is printed, worst first, as `score  path:line  name`.

mod score;

use std::process::{Command, ExitCode};

/// The highest cognitive complexity a function may have.
const LIMIT: u32 = 10;

/// Rust files git tracks or would track, without the vendored crate patches.
fn tracked_rust_files() -> Vec<String> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ])
        .args(["*.rs", ":(exclude,glob)**/patches/**"])
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8 paths")
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

struct Violation {
    path: String,
    function: score::FunctionScore,
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let paths = if arguments.is_empty() {
        tracked_rust_files()
    } else {
        arguments
    };
    let mut violations = Vec::new();
    let mut unreadable = 0;
    for path in paths {
        let scored = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|source| score::score_file(&source).map_err(|error| error.to_string()));
        match scored {
            Ok(functions) => violations.extend(
                functions
                    .into_iter()
                    .filter(|function| function.score > LIMIT)
                    .map(|function| Violation {
                        path: path.clone(),
                        function,
                    }),
            ),
            Err(error) => {
                eprintln!("{path}: {error}");
                unreadable += 1;
            }
        }
    }
    violations.sort_by(|a, b| {
        b.function
            .score
            .cmp(&a.function.score)
            .then(a.path.cmp(&b.path))
    });
    for Violation { path, function } in &violations {
        println!(
            "{:>4}  {path}:{}  {}",
            function.score, function.line, function.name
        );
    }
    if unreadable > 0 {
        eprintln!("{unreadable} files could not be read or parsed");
        return ExitCode::from(2);
    }
    if violations.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!(
        "{} functions have a cognitive complexity above {LIMIT}",
        violations.len()
    );
    ExitCode::FAILURE
}
