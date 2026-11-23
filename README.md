# Northroom

Northroom is a desktop file browser for working with AI agents. It shows a folder on your
computer. Your agents run in terminals inside the app, and Northroom shows everything they make:
pages, live HTML, video, sound, pictures, 3D models and tables.

It's free, has no accounts and generates nothing itself. Bring your own agent and model.
Your files stay on your computer.

## Install

Download the installer for your computer from the
[latest release](https://github.com/main-eng/northroom/releases/latest):

- **Mac (Apple chip):** `Northroom-<version>-arm64.zip`. Unzip it and move `Northroom.app` to
  Applications.
- **Windows (x64):** `Northroom-<version>-x64.exe`. It installs for your user only. It is
  unsigned, so Windows may warn first.
- **Linux (x86_64):** `Northroom-<version>-x86_64.AppImage`. Make it executable and run it.

Each release's `native-mac.yml`, `native-windows.yml` and `native-linux.yml` list every file's
SHA-512 (base64) to check a download against. Intel Macs aren't supported; build from source.

Then [set up your workspace](./SETUP.md): one folder for your work, rules and memory every
agent shares, and screenshots where agents can see them.

## Layout

| Path          | What                                                         |
| ------------- | ------------------------------------------------------------ |
| `apps/native` | The app for Mac, Windows and Linux: Rust core and GPUI shell |
| `tools`       | The Rust complexity check                                    |
| `scripts`     | Repository checks                                            |

## Build and run

Install Rust with [rustup](https://rustup.rs). Each program pins its toolchain. You also need:

- **Mac:** the Xcode Command Line Tools.
- **Windows:** Git for Windows (run the scripts in Git Bash) and Visual Studio Build Tools with
  the C++ workload.
- **Linux:** the packages in `LINUX_PACKAGES` in
  [`.github/workflows/gpui-shell.yml`](./.github/workflows/gpui-shell.yml).

```bash
apps/native/scripts/with-build-lock.sh apps/native/scripts/build.sh
apps/native/gpui/target/release/northroom-gpui
```

`with-build-lock.sh` keeps two builds from running at once. Packaging is in
[`apps/native/README.md`](./apps/native/README.md).

## Test

```bash
# From apps/native
scripts/with-build-lock.sh cargo test --manifest-path core/Cargo.toml --release --lib
scripts/with-build-lock.sh cargo test --manifest-path gpui/Cargo.toml --release -p kit -p shell

# From the repository root
bun install
bun run test
```

## Docs

- [`CONTEXT.md`](./CONTEXT.md): terms, and which part owns what.
- [`DECISIONS.md`](./DECISIONS.md): decisions and why.
- [`AGENTS.md`](./AGENTS.md): rules for AI agents working here.

Releases are the repository's GitHub releases. The website (northroom.com) lives in its own
repository.

## License

[Apache 2.0](LICENSE). See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md).
