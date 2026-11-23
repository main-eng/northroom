# Working in this repository

You maintain this codebase. Choose what is best for Northroom five to ten years out, even when it
means more work now. Replace a weak structure instead of patching around it. Finish what you start:
every code path, its tests and the docs it affects.

## Run the app

Build under the build lock, then run the program it prints:

```bash
apps/native/scripts/with-build-lock.sh apps/native/scripts/build.sh
apps/native/gpui/target/release/northroom-gpui
```

`apps/native/scripts/bundle.sh <folder>` makes `Northroom.app`. The website is the private repo
`main-eng/northroom-web`, and DNS is `main-eng/northroom-infra`; both sit beside this repo.

## Check your change

- App: `cargo test` for the core and the shell.
- Repository scripts: `bun run test:repo`.

Run every `cargo` command through `apps/native/scripts/with-build-lock.sh`. Several agents share
one 16 GB machine: run one Rust build at a time, and check `df -h /` before
a full gate.

Performance runs are opt-in: `cargo bench`, timing workloads, and stress fixtures. Run them only when asked in the current task. Don't add them
to ordinary checks, and don't delete them unless the owner asks.

## Don't run these unless asked in the current task

- Deploys and releases: the
  native `release-*.sh` scripts.
- Repo-wide rewrites such as `bun run format` or any `--fix`/`--write` over the whole repo. Format
  the files you touched, by name.
- Anything that writes a lint baseline or allowlist. Fix the code instead.
