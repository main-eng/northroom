import Foundation

/// What this device asks the computer (docs/phone-link.md).
public enum Ask: Equatable, Sendable {
    case pair(code: String, name: String)
    case hello
    case watch(session: String, columns: Int, lines: Int)
    case leave
    case type(session: String, text: String)
    case press(session: String, key: PressedKey)
    /// Moves through earlier output by `lines`, towards older output when positive.
    case scroll(session: String, lines: Int)

    /// The message as the JSON the computer reads.
    public func encoded() -> Data {
        let fields: [String: Any]
        switch self {
        case .pair(let code, let name): fields = ["type": "pair", "code": code, "name": name]
        case .hello: fields = ["type": "hello"]
        case .watch(let session, let columns, let lines):
            fields = ["type": "watch", "session": session, "columns": columns, "lines": lines]
        case .leave: fields = ["type": "leave"]
        case .type(let session, let text): fields = ["type": "type", "session": session, "text": text]
        case .press(let session, let key):
            fields = ["type": "press", "session": session, "key": key.rawValue]
        case .scroll(let session, let lines):
            fields = ["type": "scroll", "session": session, "lines": lines]
        }
        // Strings and whole numbers always encode.
        return (try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys])) ?? Data()
    }
}

/// The keys a phone's keyboard lacks.
public enum PressedKey: String, CaseIterable, Sendable {
    case enter, escape, tab, backspace, up, down, left, right, interrupt
}

public enum Activity: String, Codable, Sendable {
    case needsYou = "needs-you"
    case finished, working, idle
}

/// One running session.
public struct Session: Codable, Equatable, Identifiable, Sendable {
    public var id: String
    public var name: String
    public var folder: String
    public var doing: String
    public var agent: String
    public var activity: Activity
}

/// Text in one look, starting at `column`.
public struct Run: Codable, Equatable, Sendable {
    public var column: Int
    public var text: String
    public var ink: String?
    public var ground: String?
    public var bold: Bool?
    public var italic: Bool?
    public var dim: Bool?
    public var underline: Bool?
    public var strikeout: Bool?
}

/// What a watched terminal shows.
public struct Screen: Codable, Equatable, Sendable {
    public var columns: Int
    public var lines: Int
    public var ink: String
    public var ground: String
    public var rows: [[Run]]
    /// The cursor's line and column, when shown.
    public var cursor: [Int]?
}

/// What the computer tells this device.
public enum Tell: Equatable, Sendable {
    case welcome(host: String)
    /// `why` is `not-paired`, `wrong-code` or `not-understood`.
    case refused(why: String)
    case sessions([Session])
    case screen(session: String, Screen)
    case gone(session: String)

    private struct Wire: Decodable {
        var type: String
        var host: String?
        var why: String?
        var sessions: [Session]?
        var session: String?
        var screen: Screen?
    }

    /// Reads one message. `nil` for a type this version does not know, which is skipped.
    public init?(_ data: Data) {
        guard let wire = try? JSONDecoder().decode(Wire.self, from: data) else { return nil }
        switch (wire.type, wire.session, wire.screen) {
        case ("welcome", _, _): self = .welcome(host: wire.host ?? "")
        case ("refused", _, _): self = .refused(why: wire.why ?? "")
        case ("sessions", _, _): self = .sessions(wire.sessions ?? [])
        case ("screen", let session?, let screen?): self = .screen(session: session, screen)
        case ("gone", let session?, _): self = .gone(session: session)
        default: return nil
        }
    }
}
