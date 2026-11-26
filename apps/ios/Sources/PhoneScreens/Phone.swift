import Foundation
import Observation
import PhoneLink

#if canImport(UIKit)
    import UIKit
#endif

/// Everything the screens show, and the one link to the computer behind it.
@MainActor
@Observable
public final class Phone {
    public enum State: Equatable {
        case connecting, connected, unreachable
    }

    /// The computer this device is paired with. `nil` shows the scan screen.
    public private(set) var computer: Computer?
    public private(set) var state = State.connecting
    public private(set) var sessions: [Session] = []
    /// The watched terminal's screen, once the computer has sent it.
    public private(set) var screen: Screen?
    /// Set when the watched session ended.
    public private(set) var gone = false
    /// Why the last pairing did not work, for the scan screen.
    public private(set) var notice: String?
    public private(set) var pairing = false

    private let keep = Keep()
    private var link: Link?
    private var listening: Task<Void, Never>?
    private var watching: (session: String, columns: Int, lines: Int)?

    public init() {
        computer = keep.computer
    }

    // MARK: Pairing and connecting

    /// Pairs with the computer a scanned or opened pairing link names.
    public func pair(link text: String) async {
        guard !pairing, computer == nil, let invite = Invite(link: text) else { return }
        pairing = true
        notice = nil
        await connect(to: Computer(invite: invite), code: invite.code)
        pairing = false
    }

    /// Connects when the app comes to the front.
    public func wake() async {
        guard let computer, link == nil else { return }
        await connect(to: computer, code: nil)
    }

    /// Lets the link go when the app leaves the front, so the computer's terminal takes its own
    /// size back.
    public func rest() async {
        await drop()
        if computer != nil { state = .connecting }
    }

    public func forget() async {
        await drop()
        keep.forget()
        computer = nil
        sessions = []
        watching = nil
        screen = nil
    }

    private func drop() async {
        listening?.cancel()
        listening = nil
        let old = link
        link = nil
        await old?.close()
    }

    private func connect(to computer: Computer, code: String?) async {
        await drop()
        state = .connecting
        let secret = keep.secret()
        let name = Phone.deviceName
        do {
            let (link, host) = try await within(seconds: 15) {
                let link = try await Link(secret: secret, computer: computer)
                return (link, try await link.greet(code: code, deviceName: name))
            }
            var known = computer
            if !host.isEmpty { known.name = host }
            keep.computer = known
            self.computer = known
            self.link = link
            state = .connected
            listening = Task { await self.listen(to: link) }
            if let watching {
                try? await link.ask(
                    .watch(session: watching.session, columns: watching.columns, lines: watching.lines))
            }
        } catch LinkError.refused {
            await turnedAway(pairing: code != nil)
        } catch {
            if code != nil { notice = "Couldn't reach the computer. Try the code again." }
            state = .unreachable
        }
    }

    /// The computer refused: the code is spent, or it has forgotten this device.
    private func turnedAway(pairing: Bool) async {
        await forget()
        notice =
            pairing
            ? "That code no longer works. Choose Connect Phone… again for a new one."
            : "The computer no longer knows this phone. Pair again."
    }

    private func listen(to link: Link) async {
        while !Task.isCancelled, let tell = try? await link.next() {
            take(tell)
        }
        guard !Task.isCancelled, self.link === link, let computer else { return }
        // The link ended on its own: try once more before saying so.
        self.link = nil
        await connect(to: computer, code: nil)
    }

    private func take(_ tell: Tell) {
        switch tell {
        case .sessions(let running): sessions = running
        case .screen(let session, let shown) where session == watching?.session: screen = shown
        case .gone(let session) where session == watching?.session: gone = true
        default: break
        }
    }

    // MARK: One session

    public func watch(session: String, columns: Int, lines: Int) async {
        if watching?.session != session {
            screen = nil
            gone = false
        }
        watching = (session, columns, lines)
        try? await link?.ask(.watch(session: session, columns: columns, lines: lines))
    }

    public func leave() async {
        watching = nil
        screen = nil
        gone = false
        try? await link?.ask(.leave)
    }

    /// Types `text` into the watched session and presses Enter.
    public func send(_ text: String) async {
        guard let session = watching?.session else { return }
        if !text.isEmpty {
            try? await link?.ask(.type(session: session, text: text))
        }
        try? await link?.ask(.press(session: session, key: .enter))
    }

    public func press(_ key: PressedKey) async {
        guard let session = watching?.session else { return }
        try? await link?.ask(.press(session: session, key: key))
    }

    private static var deviceName: String {
        #if canImport(UIKit)
            UIDevice.current.name
        #else
            Host.current().localizedName ?? "Device"
        #endif
    }
}

/// `work`'s result, or `LinkError.closed` when it takes longer than `seconds`.
private func within<T: Sendable>(
    seconds: Double, _ work: @escaping @Sendable () async throws -> T
) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask(operation: work)
        group.addTask {
            try await Task.sleep(for: .seconds(seconds))
            throw LinkError.closed
        }
        defer { group.cancelAll() }
        guard let first = try await group.next() else { throw LinkError.closed }
        return first
    }
}
