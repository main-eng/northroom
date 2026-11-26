# Phone link

How a phone, or any device, connects to a computer running Northroom: lists its running
sessions, watches one terminal and types into it. Anyone may build a device or a host that
speaks this. The rules are in `apps/native/core/src/remote/`, the connection in
`apps/native/crates/northroom-link/`, and a working device in that crate's `Client`.

Protocol number: `northroom/1`. A change an older side can't read takes the next number; adding
a message type or a field does not.

## Connection

- Each side has an Ed25519 key pair and is known by its public key (32 bytes, written as 64 hex
  digits).
- The connection is [iroh](https://iroh.computer) 1.x: QUIC, encrypted end to end between the
  two keys, direct where the networks allow it and through a relay where they don't. A relay
  sees who talks to whom and how much, never what.
- The ALPN is `northroom/1`.
- The device opens one bidirectional stream. Each message on it, either way, is its length as
  four bytes, big-endian, then that many bytes of UTF-8 JSON. The longest is 4 MiB.
- No address is published or looked up anywhere. A device dials the address its pairing link
  gave it.

## Pairing

The computer shows a link as a QR code:

```
northroom://pair?key=<64 hex>&code=<code>&name=<computer's name>&relay=<relay URL>&at=<ip:port>&at=…
```

- Values are percent-encoded. `relay` is absent when the computer uses none; there are up to
  four `at`, the computer's addresses on its own networks. Skip a name you don't know.
- The code works once, for five minutes, and stops working when the computer takes the link off
  screen.
- The device dials `key` at `relay` and the `at` addresses, then sends `pair`. From then on the
  computer lets that device's key in without a code, until the person forgets the device.

## What a device sends

| Message                                                     | Meaning                                                                                                                    |
| ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `{"type":"pair","code":"…","name":"My phone"}`              | Pair this device. `name` is what the computer lists it as.                                                                 |
| `{"type":"hello"}`                                          | A paired device connected. Answered with `welcome`, then `sessions`.                                                       |
| `{"type":"watch","session":"<id>","columns":48,"lines":30}` | Show this session's terminal, fitted to this grid (20 to 300 columns, 5 to 200 lines), until another `watch` or a `leave`. |
| `{"type":"leave"}`                                          | Stop watching. The terminal takes back the size of its view on the computer.                                               |
| `{"type":"start"}`                                          | Make a terminal at the top of the drive and start its shell. Answered with `sessions`, then `started`.                     |
| `{"type":"type","session":"<id>","text":"…"}`               | Type text, as a paste. It does not press Enter.                                                                            |
| `{"type":"press","session":"<id>","key":"enter"}`           | Press one key: `enter`, `escape`, `tab`, `backspace`, `up`, `down`, `left`, `right` or `interrupt` (Control-C).            |
| `{"type":"scroll","session":"<id>","lines":3}`              | Move through earlier output as the computer's wheel does, towards older output when positive; at most 500 lines a message. |

A device that is not paired may send only `pair`. Anything else, or a message that can't be
read, is answered with `refused` and the link closes.

## What the computer sends

| Message                                           | Meaning                                                                                           |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `{"type":"welcome","host":"<computer's name>"}`   | The device may stay.                                                                              |
| `{"type":"refused","why":"not-paired"}`           | The device may not, and the link closes. `why` is `not-paired`, `wrong-code` or `not-understood`. |
| `{"type":"sessions","sessions":[…]}`              | Every open terminal, most urgent first, a shell at its prompt as `idle`. Sent after `hello` and whenever it changes. |
| `{"type":"screen","session":"<id>","screen":{…}}` | The watched terminal's screen. Sent after `watch` and whenever it changes.                        |
| `{"type":"gone","session":"<id>"}`                | The watched session ended.                                                                        |
| `{"type":"started","session":"<id>"}`             | The session a `start` made. It is in the `sessions` sent just before.                             |

A session:

```json
{
  "id": "…",
  "name": "Build",
  "folder": "Work / App",
  "doing": "Fixing the tests",
  "agent": "Claude Code",
  "activity": "needs-you"
}
```

`activity` is `needs-you`, `finished`, `working` or `idle`. `agent` and `doing` may be empty.

A screen:

```json
{
  "columns": 48,
  "lines": 30,
  "ink": "#eeeef0",
  "ground": "#111111",
  "cursor": [3, 7],
  "rows": [
    [{ "column": 0, "text": "$ cargo test" }],
    [],
    [{ "column": 0, "text": "ok", "ink": "#116329", "bold": true }]
  ]
}
```

- `rows` has one entry a line, top to bottom. Each is runs of text in one look, starting at
  `column`; a wide character takes two columns. Blank ends of lines are left out.
- A run may have `ink` and `ground` (`#rrggbb`; the screen's own when absent) and `bold`,
  `italic`, `dim`, `underline` and `strikeout` (false when absent).
- `cursor` is its line and column, absent when it is hidden or scrolled out of view.

## Limits

- One session is watched at a time per device. Two devices watching one session share the grid
  of whichever asked last.
- The whole screen is sent on each change, at most about seven times a second.
- Scrolling moves the terminal's one view: the computer's window scrolls with the device, and
  typing shows the newest output again. A full-screen program gets the scroll as its own wheel
  or arrow keys.
- There is no file browsing yet.
