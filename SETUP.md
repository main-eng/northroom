# Set up your workspace

Northroom shows one folder, the drive, and everything in it is shared by you and every agent you
run. These steps make that folder the home for your work. They are written for an agent to follow
with you: do one step at a time, show the plan, and wait for a yes before changing anything.
Never delete a file; move it.

## 1. Choose the drive

Ask which the person wants:

- **A new folder** they name (for example `~/Workspace`). Best when their files are spread out.
- **An existing folder**, such as `~/Documents`.

If files are spread out, list what is in Desktop, Documents, Downloads and other folders in their
home (names and sizes, not contents), propose a few top-level folders (projects, notes,
screenshots), and move only what they approve. If another program expects something at its old
path, leave a link there.

Then set the drive in Northroom's settings file and tell them it takes effect at the next start:

- Mac and Linux: `~/.config/northroom/settings.json`
- Windows: `%APPDATA%\Northroom\settings.json`

```json
{ "drive": "/Users/you/Workspace" }
```

Keep every other key in the file as it is.

## 2. One set of rules for every agent

Write `AGENTS.md` at the top of the drive with what every agent should know: who the person is,
how they like to work, and where things go. Codex, Cursor and most agents read it. For an agent
that reads another file, point that file at it: Claude Code's `CLAUDE.md` holds one line,
`@AGENTS.md`.

## 3. Memory every agent shares

Make a `Memory` folder: one Markdown file per fact, plus `Memory/README.md` listing them. Add to
`AGENTS.md`: read the list before starting, and save anything worth keeping as a new file. Offer
to bring in what agents already remember (Claude Code keeps its memory under `~/.claude`), with
the person's OK.

## 4. Skills

Make a `Skills` folder for how-to pages agents follow (a release checklist, a writing style), and
tell agents about it in `AGENTS.md`.

## 5. Screenshots into the drive

So agents can see them, save screenshots into `<drive>/Screenshots`:

- Mac: `defaults write com.apple.screencapture location "<drive>/Screenshots"`, then
  `killall SystemUIServer`.
- Windows: in File Explorer, open Pictures, right-click Screenshots, Properties, Location, Move.
- Linux: it depends on the desktop; ask the person which screenshot tool they use.

Optionally point the browser's download folder at `<drive>/Downloads` the same way.

## 6. Keep secrets out

Every agent can read the drive. Keep passwords, API keys and `.env` files outside it, or name
them in `AGENTS.md` as files no agent opens.

## 7. Backups and terminals

Check the drive is backed up (Time Machine, File History or a synced folder). Then, in
Northroom, make a terminal in the drive for each agent the person uses and name it after the
agent.

## 8. Automations (optional)

Northroom already shows whether each agent is working, waiting or done; that needs no setup. If
the person wants more (formatting files after edits, blocking risky commands, running checks
before an agent says it is done), set it up in that agent's own settings, and explain what each
one runs before adding it.
