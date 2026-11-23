# Contributing

Thanks for helping.

1. Read the [README](README.md) to build and run the app, and [DECISIONS.md](DECISIONS.md) for
   the rules the code follows.
2. For anything bigger than a fix, open an issue first so we can agree on the approach.
3. Keep each pull request to one change, merged into `master`. When you replace something, delete
   the old version in the same change.
4. Run the checks for what you touched: `bun run quality`, `bun run test`, and `cargo test` in the
   crate you changed (through `apps/native/scripts/with-build-lock.sh`).
5. Describe what changes for people who use Northroom.

Contributions are licensed under [Apache 2.0](LICENSE).
