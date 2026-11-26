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

    /// A listed folder: its name and what is in it, or that it is gone.
    public enum Folder: Equatable {
        case listed(name: String, items: [Item])
        case gone
    }

    /// The key `folders` keeps the top of the drive under.
    public static let top = ""

    /// The computer this device is paired with. `nil` shows the scan screen.
    public private(set) var computer: Computer?
    public private(set) var state = State.connecting
    public private(set) var sessions: [Session] = []
    /// The watched terminal's screen, once the computer has sent it.
    public private(set) var screen: Screen?
    /// Set when the watched session ended.
    public private(set) var gone = false
    /// The session this device just started, for the sessions screen to open, once.
    public private(set) var started: String?
    /// The folders listed so far, by id; the top of the drive is `Phone.top`. A folder that is
    /// no longer in the drive is `.gone`.
    public private(set) var folders: [String: Folder] = [:]
    /// Why the last pairing did not work, for the scan screen.
    public private(set) var notice: String?
    public private(set) var pairing = false

    private let keep = Keep()
    private var link: Link?
    private var listening: Task<Void, Never>?
    private var watching: (session: String, columns: Int, lines: Int)?
    /// The last key sent, which the next one waits for.
    private var sending: Task<Void, Never>?

    public init() {
        computer = keep.computer
    }

    // MARK: Pairing and connecting

    /// Pairs with the computer a scanned or opened pairing link names. A computer paired before
    /// is forgotten first: a new code is the person choosing this one.
    public func pair(link text: String) async {
        guard !pairing, let invite = Invite(link: text) else { return }
        pairing = true
        if computer != nil { await forget() }
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
        folders = [:]
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
        case .started(let session): started = session
        case .folder(let folder, let name, let items):
            folders[folder ?? Phone.top] = .listed(name: name, items: items)
        case .noFolder(let folder): folders[folder ?? Phone.top] = .gone
        default: break
        }
    }

    // MARK: One session

    /// Asks the computer for a new terminal. `started` names it when it runs.
    public func start() async {
        try? await link?.ask(.start)
    }

    /// The started session is open on screen.
    public func openedStarted() {
        started = nil
    }

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
    /// Moves the watched terminal through its earlier output, towards older output when
    /// `lines` is positive.
    public func scroll(lines: Int) async {
        guard let session = watching?.session, lines != 0 else { return }
        try? await link?.ask(.scroll(session: session, lines: lines))
    }

    public func press(_ key: PressedKey) async {
        guard let session = watching?.session else { return }
        try? await link?.ask(.press(session: session, key: key))
    }

    /// Types what the keyboard gave into the watched terminal, in the order it was typed.
    public func typed(_ text: String) {
        guard let session = watching?.session else { return }
        send(text == "\n" ? .press(session: session, key: .enter) : .keys(session: session, text: text))
    }

    /// Erases the character before the cursor in the watched terminal.
    public func erased() {
        guard let session = watching?.session else { return }
        send(.press(session: session, key: .backspace))
    }

    /// Sends `ask` after everything sent before it: keys must arrive in the order typed.
    private func send(_ ask: Ask) {
        let before = sending
        sending = Task { [link] in
            await before?.value
            try? await link?.ask(ask)
        }
    }

    // MARK: Files

    /// Asks for what is in `folder`, or at the top of the drive for `nil`. `folders` gains it.
    public func list(folder: String?) async {
        try? await link?.ask(.list(folder: folder))
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
