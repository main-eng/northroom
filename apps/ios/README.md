# Phone

The phone's side of the phone link (ADR-0016). The messages are in
[docs/phone-link.md](../../docs/phone-link.md).

```
Sources/PhoneLink/         reading a pairing link, the messages, the connection (no screens)
Sources/PhoneScreens/      the three screens: scan, sessions, one session
Sources/phone-link-check/  a device with no screen, for checking a running Northroom
Tests/PhoneLinkTests/      the link and the messages, read and written
App/                       the app itself: its entry and the `northroom://` link it opens
UITests/                   the whole app against a running Northroom
Northroom.xcodeproj        builds App and UITests; the rest is the Swift package
```

## Run it

Open `Northroom.xcodeproj` in Xcode, choose a simulator or your phone, and run. On a phone, set
your team under Signing & Capabilities first.

The simulator has no camera. Pair it by opening the pairing link instead:

```bash
xcrun simctl openurl booted '<northroom://pair?… link>'
```

## Check a running Northroom

Choose Northroom > Connect Phone…, read the QR code's link with any QR reader, then:

```bash
swift build
.build/debug/phone-link-check '<northroom://pair?… link>' /tmp/phone.key
```

The first run pairs with the link's code and lists the sessions. A second run with the same key
file is let in by its key alone. A run without the key file is turned away, since the code works
once.

## Tests

```bash
swift test
```

The end-to-end test needs a running Northroom showing Connect Phone…, with one session that is
working or needs you. Without the link it is skipped.

```bash
TEST_RUNNER_NR_PAIR_LINK='<northroom://pair?… link>' xcodebuild test \
  -project Northroom.xcodeproj -scheme Northroom \
  -destination 'platform=iOS Simulator,name=iPhone 17'
```
