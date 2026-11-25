// A device with no screen, for checking a running Northroom from a terminal:
//
//   swift run phone-link-check '<pairing link>'            pair, then list the sessions
//   swift run phone-link-check '<pairing link>' <keyfile>  keep this device's key in a file,
//                                                          so a second run reconnects by key
//
// It prints what the computer says and exits 0 when it was let in.
import Foundation
import PhoneLink

let arguments = CommandLine.arguments
guard arguments.count >= 2, let invite = Invite(link: arguments[1]) else {
    print("usage: phone-link-check '<northroom://pair?… link>' [keyfile]")
    exit(2)
}

/// This device's key: the one kept in `path`, or a new one, kept there for next time.
func secret(at path: String?) -> (key: Data, known: Bool) {
    if let path, let kept = FileManager.default.contents(atPath: path), kept.count == 32 {
        return (kept, true)
    }
    let fresh = Link.newSecret()
    if let path {
        FileManager.default.createFile(atPath: path, contents: fresh, attributes: [.posixPermissions: 0o600])
    }
    return (fresh, false)
}

let (key, known) = secret(at: arguments.count > 2 ? arguments[2] : nil)
do {
    let link = try await Link(secret: key, computer: Computer(invite: invite))
    let host = try await link.greet(code: known ? nil : invite.code, deviceName: "phone-link-check")
    print(known ? "let in by key: \(host)" : "paired with: \(host)")
    if case .sessions(let sessions) = try await link.next() {
        print("sessions: \(sessions.count)")
        for session in sessions {
            print("  \(session.activity.rawValue)\t\(session.name)\t\(session.agent)")
        }
    }
    await link.close()
} catch {
    print("turned away: \(error)")
    exit(1)
}
