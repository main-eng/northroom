# Phone

The phone's side of the phone link (ADR-0016). The messages are in
[docs/phone-link.md](../../docs/phone-link.md).

```
Sources/PhoneLink/         reading a pairing link, the messages, the connection (no screens)
Sources/phone-link-check/  a device with no screen, for checking a running Northroom
Tests/PhoneLinkTests/      the link and the messages, read and written
```

The app's screens are not written yet. They need Xcode, to build for a phone and to be seen.

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

`swift test` needs Xcode: the command-line tools alone have no test framework that links.
