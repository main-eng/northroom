# Decisions

Decisions the code can't explain by itself. Code cites them as `ADR-NNNN`.
Change one only with evidence (a measurement, a failing case or a simpler design), and update
this file in the same change. Don't retry a rejected option without evidence that answers the
reason it was rejected.

## ADR-0001: One Rust core decides; one GPUI shell draws, on all three systems

- Every rule lives in the core. A rule found in the shell moves to the core.
- Web views are only for live HTML and 3D model stills. Never for lists, the board, settings, the editor, menus or the terminal.
- The terminal is drawn by the shell like every other view, in GPUI's frame: the core hands it a frame of what to draw (`terminal::frame`, in Northroom's own types) and the emulator library stays behind `terminal::engine`. The grid and the process are resized at most once a frame, and only when a whole cell is gained or lost.
- English only. A language ships complete or not at all.
- No screenshot-comparison tests. They cost more to keep than they caught.
- **Why:** native is faster, leaner and smoother than a browser shell. A shell with no logic keeps the toolkit replaceable.
- **Rejected:**
  - Electron: the largest start-up and memory cost.
  - A native shell per OS: every view built three times.
  - GPUI `surface` for video: a second GPU device, a per-frame UI wait, no HDR.
  - A long-lived GPUI fork: patches stay small and go upstream.
  - A terminal renderer of its own behind a hole in GPUI's frame: the two fell out of step while resizing or moving, so text flickered.

## ADR-0002: Work is ordinary files in one local folder

- Store nothing about the drive's files: no catalog, tags, colours, manual order or usage. Ids come from device and inode, in memory only. Only view state is kept.
- List every file except `.DS_Store`. No allowlist, no conversion, no starter files. New abilities arrive as new file kinds.
- Nothing is overwritten or deleted for good. With no system trash, delete refuses.
- One person, one drive. No teams, no search index.
- Change review (a Changes tab) keeps a pruned, content-addressed snapshot store in the app's data folder (14 days, at most 2 GB), never beside the files and never in the person's repository. Beside it, a small JSON record per root keeps what the list needs across restarts (each listed file's baseline and credited terminal files, and what the person kept of each file); its entries expire with the store's 14 days, and pruning spares the snapshots a record names. A repository marks where a root's changes are gathered, and the tab's Uncommitted and vs main scopes read it through the person's own `git` with read-only commands and no optional locks (`rev-parse`, `symbolic-ref`, `diff`, `ls-files`, `show`, `merge-base`), never writing to the repository; without `git` those scopes are hidden. Undo writes the person's files only after moving their current version to the system Trash.
- **Why:** plain files work with every agent and tool. Anything stored beside them drifts when an agent changes them. Holding no content means nothing to sell, secure or delete.
- **Rejected:**
  - A catalog beside the drive: it drifts.
  - Hosted storage: quotas, deletion, and files leave the computer.
  - An app-owned store or custom formats: the folder stops being the truth, and models don't know the format.

## ADR-0003: No authoring format of our own; agents use their own tools

- Northroom is a workspace. It shows what agents make; it has no timeline, film or layer format.
- **Why:** agents can't use a format they never saw in training. Open tools (Remotion, FFmpeg, HTML) improve without us.
- **Rejected:**
  - A folder of timed HTML pages the app plays and exports: only agents taught the format could use it.
  - A native layer format: agents write it worse than code.
  - Timeline or keyframe controls: a second way to make the agent's edits.
  - Server rendering: media leaves the computer.

## ADR-0004: Media goes through the system's codecs; nothing is bundled

- No FFmpeg. One decode path per system, no fallback.
- **Why:** system decoders are hardware-accelerated, licensed and kept current. Direct use matched a library decode frame for frame and ran several times faster than FFmpeg's wrapper. Our own resampler makes sound identical on every system.
- **Rejected:**
  - Bundled FFmpeg: GPL and size. Linux FFmpeg has no stable interface.
  - FFmpeg wrapping platform decoders: several times slower.
  - The platform reader's decode and resample: on the Mac it fails on some H.264 and AV1 streams, and its audio lags.
  - A fallback decoder: it hides wrong frames.

## ADR-0005: People bring their own agent

- No built-in assistant, skills, models or MCP server.
- A `.nterm` file names nothing to run. Opening a received one starts only the person's shell.
- Never pass on the app's environment, read agent credentials, edit agent settings, or write into PATH or folders.
- Known limits: Claude Code older than 2.1.280 ignores the plugin. Codex shows approvals only through OSC 9. Codex can't be hooked without editing its config (no environment-only hooks or `notify`), so its changes are credited by timing only. Agents inside tmux or ssh aren't seen.
- **Why:** people already pay for their agent. The terminal works for every vendor; not every agent speaks MCP. Bundled models freeze at release.
- **Rejected:**
  - A built-in assistant or vendor-SDK chat: our cost, plus vendor approval.
  - Bundled models: a runtime, a download and a licence per platform.
  - Skills or instruction files in folders: copies that go stale.
  - Northroom-only command-line tools or a local MCP server: they drift from the files.
  - Split panes, side panels or session sidebars: layouts to maintain, and they hide waiting sessions.

## ADR-0006: Production is a static site and GitHub releases

- The website is static files on Cloudflare, in the private repo `main-eng/northroom-web`. The private repo `main-eng/northroom-infra` declares its DNS in Terraform.
- The app's installers and update files are this repository's GitHub releases. The download page asks GitHub's API for the latest release.
- The app reads its update feed at `api.northroom.com/api/download/update`, which a Cloudflare rule redirects to `releases/latest/download`. Every address built into the app is on `northroom.com`: an installed app can't be changed, a redirect can.
- Nothing runs on push. Releases go out by hand from `master`.
- Dashboards are read-only. Secret values never enter Terraform state.
- **Why:** a public repository has nothing to guard, so nothing needs to stand between people and the release files. No backend is cheaper and safer than a small one.
- **Rejected:**
  - A download server: only needed when releases are private.
  - GitHub's address built into the app (1.3.0 only): moving or closing the repository would end updates for every installed copy.
  - Serverless functions: queues, a pooler and cron, and their limits shape the code.
  - A staging environment: nothing to stage.

## ADR-0007: Northroom keeps no data

- No database and no backend.
- **Why:** a database is the costliest part to run and secure.
- **Rejected:**
  - A database for feedback or counts: email and download counts cover it.

## ADR-0008: No accounts

- **Why:** sign-in is where people leave, and data never collected can't leak.
- **Rejected:**
  - Email or Google sign-in: nothing to protect.
  - Accounts to meter free generation: unbounded cost, and it invites duplicate accounts.
  - An install id: it tracks people who never agreed to it.

## ADR-0009: Free, and generates nothing

- **Why:** usage costs grow with users and nobody pays them. The agent already picks the model. Billing code is the hardest to keep correct.
- **Rejected:**
  - A prepaid balance: needs accounts and a database.
  - Free hosted generation: anyone can spend it.
  - People's API keys in the app: that rebuilds the agent.
  - Subscriptions: they charge loyal users most, and their rules breed bugs.

## ADR-0010: Updates install only what the app's own signer signed

- Install only if three checks pass: the SHA-512 matches the feed, the signing key matches the running app's (the key, not the certificate name), and the signed version equals the offered version.
- Publishing is the owner's manual step. Scripts never upload.
- The app takes only the newest `v<version>` release, so a release holds every platform's files. Each version must be higher than the last.
- Never open a release build from its output folder: it has the installed app's bundle id.
- Mac: always sign with the same Apple team. A notarytool 403 "required agreement is missing" means accepting the Developer Program agreement, not replacing the certificate.
- Windows: a renewed certificate needs a dual-signed release before the old one expires. An unsigned release needs the owner's explicit request, passing CI install and SHA checks, a download labelled "unsigned, manual updates", and no published channel file.
- Linux: keep an offline backup of the minisign key, or installed apps can never update in place.
- **Why:** each check closes a gap. A certificate with the same name and a new key is a different signer. The version check stops older builds being replayed. A manual publish prevents accidental releases.
- **Rejected:**
  - MSIX: needs a trusted certificate, and its virtual filesystem breaks terminals.
  - Per-user MSI: can't update in place.
  - NSIS: Inno has a per-user mode and signing built in.
  - A `.deb` with no repository: can't update itself.
  - Publishing from CI: one wrong tag reaches everyone.

## ADR-0011: Our own code, typed at the edges, held by gates without exceptions

- One path: delete a replaced part in the same change. No readers for old formats. Old saved work may break; preferences fall back field by field.
- Outside contracts aren't legacy.
- Every dependency states its reason. Crypto and codecs stay dependencies.
- Every Rust function stays at cognitive complexity 10 or below (`tools/cognitive-complexity`).
- No lint baseline. A new lint rule means fixing every finding first.
- **Why:** a dependency is code nobody here chose. Two paths drift apart. Baselines only grow. Timing tests are noise.
- **Rejected:**
  - Patching modules in tests: that tests the patch.
  - `develop` and `staging` branches: they drift.
  - Split repositories: a change across programs stops being one commit.

## ADR-0012: No staff tools

- **Why:** a privileged production tool is only worth its risk when there is something to operate.
- **Rejected:**
  - A staff portal: a privileged surface for one operator.

## ADR-0013: Glyphs are Lucide icons, drawn at the weight of Inter

- **Why:** a 1.5/24 stroke at 14–16 pt matches Inter's stems at 13 pt. Filled variants are a second icon to learn. Changing the typeface means re-checking the weight.
- **Rejected:**
  - Phosphor: too faint.
  - Solar: too heavy.
  - SF Symbols: Apple-only licence.

## ADR-0014: Settings are one file people and their agents edit

- A `drive` change applies at the next start.
- **Why:** an agent can change settings by editing a file.
- **Rejected:**
  - A file per area: nobody knew where to look.
  - Applying a drive change at once: it kills the editing agent's terminal mid-turn.

## ADR-0015: Northroom draws every file itself

- Every kind of file opens inside Northroom, drawn by its own code, the same on all three systems. Nothing is handed to another application.
- The core reads each format into Northroom's own types and the shell draws them (ADR-0001). Formats that share a container share its reader: `package` (zip and XML) serves Office and OpenDocument files, `compound` serves Office 97–2003. Documents and slides become a `deck` of pages, spreadsheets, contacts and events a table, 3D files a `.glb` the still renderer draws, and fonts a specimen drawn from their own outlines.
- A view shows a file; it does not edit it. It reads right: the same words, numbers, pictures and positions, not a copy of another program's line breaks.
- Nothing goes to Quick Look. It was a stopgap while formats gained views, and it was removed when the last one did. A new kind of file gets its own view or shows its details.
- PDF is drawn by the core's own renderer (`pdf`): it reads the file's objects, fonts and colour, and rasterizes each page's marks into layers off the UI thread. A plain JPEG or JPEG 2000 on a page stays with the system's decoders, as other media does (ADR-0004), cut to a rectangular clip. A JPEG that is see-through, masked, turned or clipped to a shape is decoded in the core and drawn into the page. JPEG 2000, JBIG2 and CCITT fax pictures (scanned pages) are decoded in the core, with the hayro project's JPEG 2000 and JBIG2 decoders and the `fax` crate's code tables, since not every system has decoders for them.
- A reader works from the format's specification or its reference implementation, and is checked against files another program wrote.
- **Why:** off the Mac a file without a view only shows its details, and on the Mac Quick Look draws a picture the app can't select from, search or keep the same across systems.
- **Rejected:**
  - Quick Look for good: Mac only, read as a picture.
  - Opening the file in its own application: the person leaves Northroom, and needs that application.
  - Converting to HTML for the web view: ADR-0001 keeps web views to live pages.
  - PDFKit on the Mac and the web view elsewhere for PDF: two renderers that drew differently, neither of them ours.
  - A bundled PDF library (PDFium, MuPDF): a C dependency to build on three systems, and MuPDF's AGPL licence.
  - A bundled office suite: its size, its licence and its release cycle.

## Design rules

- Monochrome chrome: accents use the text colour. Hue only comes from the person's choice. Red means loss.
- No pure white or black. Borders are the text colour at 10%. Hover and press overlays are 8% and 12%.
- Three text tiers, never four. Meet contrast by moving the colour, not with a warning.
- Corners: chrome square, controls 6, surfaces 10 or 14, cards 12. Shadows only on floating things.
- The title bar is one 42 px row with an empty middle for dragging. A side pane takes at most an equal share of its row.
- Toolbars fold overflow into a More menu. No warning banners. Deleting a named thing asks first.
- `parse_x` decodes, `validate_x` checks, `normalize_x` maps a T to a T.

## Open

- **Cloud cleanup.** Done on 2026-11-21 except the download Worker (`northroom-server`), its container and its one secret, which go once GitHub releases serve downloads. Then apply `northroom-infra`; its first apply turns on the www redirect, firewall and rate limit.
- **Decide what makes Northroom 10x better** before the next release.
