# App

The Rust core and the GPUI shell for Mac, Windows and Linux (ADR-0001).

```
core/      every rule, state, layout and word, behind a typed boundary
gpui/      shell/ (the app, its icon and entitlements), kit/ (components),
           webhost/ (live HTML pages and 3D stills), mac-tools/ (helpers for the checks)
crates/    libraries only the core uses: northroom-media, and northroom-link (a paired phone's
           connection, ADR-0016)
scripts/   build, package, release and checks
```

## Build and package

```bash
scripts/with-build-lock.sh scripts/build.sh   # build
gpui/target/release/northroom-gpui            # run
scripts/bundle.sh <folder>                    # Northroom.app, signed ad hoc
scripts/package-windows.sh <folder>           # Windows setup (Git Bash)
scripts/package-linux.sh <folder>             # Linux AppImage
```

Releases use `scripts/release-mac.sh`, `release-windows.sh` and `release-linux.sh`. The rules
they can't check are in ADR-0010.

## Debugging

- `NR_SNAPSHOT_DIR=<folder>` saves a picture of the window.
- `NR_AUTOPLAY=1` starts playback.
- `gpui/target/release/window-time 6 "$PWD/gpui/target/release/northroom-gpui"` times the first
  window from outside the app. Build it with `cargo build --release -p mac-tools` in `gpui`,
  through the build lock.
