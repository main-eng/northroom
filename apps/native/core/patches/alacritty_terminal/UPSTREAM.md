# alacritty_terminal: a Windows session gets its environment whole

This folder is `alacritty_terminal` 0.26.0 as published on crates.io (its `src`, manifest, licence
and readme; the reference tests, 46 MB of recordings, are left out with the manifest's `[[test]]`
entry), with one change to `convert_custom_env` in `src/tty/windows/conpty.rs`. Nothing else
differs.

## Why

On Windows, `tty::new` hands the child process an environment block made of `Options::env` and
then every variable of the calling process that `Options::env` does not name
(`std::env::vars_os()`). A terminal's session starts from a clean environment on every platform,
as a new Terminal.app window does (`core/src/terminal/launch.rs`): whatever the app itself was
started with must not leak into the person's shell. On a Mac and Linux the core starts the shell
through `/usr/bin/env -i` with only the variables it allows; on Windows there is no such program,
and the merge cannot be turned off.

## The change

The loop that appends the calling process's variables is removed: a non-empty `Options::env` is
the child's whole environment. An empty one still means "inherit" (`lpEnvironment` null), as
before. The core always passes a whole environment on Windows: the one Windows gives the
signed-in person afresh (`CreateEnvironmentBlock` with `bInherit` false), the variables the
terminal keeps from the app (`terminal::launch`'s list), and Northroom's own
(`core/src/terminal/launch.rs`).

```diff
-    // Pull the current process environment after, to avoid overwriting the user provided one.
-    for (inherited_key, inherited_value) in std::env::vars_os() {
-        if all_env_keys.insert(inherited_key.to_ascii_uppercase()) {
-            add_windows_env_key_value_to_block(
-                &mut converted_block,
-                &inherited_key,
-                &inherited_value,
-            );
-        }
-    }
```

Upstream could take it as an option (`Options::inherit_env`, default true) instead. No pull
request has been opened.

## Re-applying

When the core moves to a newer `alacritty_terminal`, copy that version's crate here the same way
and remove the same loop, or delete this folder and the `[patch.crates-io]` entries in
`core/Cargo.toml` and `gpui/Cargo.toml` once upstream lets the caller turn the merge off.
