import Foundation

/// What this device asks the computer (docs/phone-link.md).
public enum Ask: Equatable, Sendable {
    case pair(code: String, name: String)
    case hello
    case watch(session: String, columns: Int, lines: Int)
    case leave
    /// Makes a terminal on the computer and starts its session.
    case start
    case type(session: String, text: String)
    /// Types `text` as keys pressed one after another, not as a paste.
    case keys(session: String, text: String)
    /// Lists what is in `folder`, or at the top of the drive for `nil`.
    case list(folder: String?)
    /// Reads up to `length` bytes of `file` from byte `from`.
    case read(file: String, from: Int, length: Int)
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
        case .start: fields = ["type": "start"]
        case .type(let session, let text): fields = ["type": "type", "session": session, "text": text]
        case .keys(let session, let text): fields = ["type": "keys", "session": session, "text": text]
        case .list(let folder?): fields = ["type": "list", "folder": folder]
        case .list(nil): fields = ["type": "list"]
        case .read(let file, let from, let length):
            fields = ["type": "read", "file": file, "from": from, "length": length]
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

/// One file or folder in a listed folder.
public struct Item: Codable, Hashable, Identifiable, Sendable {
    public var id: String
    public var name: String
    /// `folder`, `page`, `terminal`, `image`, `video`, `audio` or `file`.
    public var kind: String
    /// A file's size in bytes; absent for a folder.
    public var size: Int?
    /// A file's extension without the dot; absent for a folder or a bare name.
    public var fileExtension: String?

    public init(id: String, name: String, kind: String, size: Int?, fileExtension: String? = nil) {
        self.id = id
        self.name = name
        self.kind = kind
        self.size = size
        self.fileExtension = fileExtension
    }

    private enum CodingKeys: String, CodingKey {
        case id, name, kind, size
        case fileExtension = "extension"
    }

    public var isFolder: Bool { kind == "folder" }

    /// The file's name on disk: the computer shows a page and a terminal without their
    /// extension.
    public var fileName: String {
        switch kind {
        case "page": name + ".md"
        case "terminal": name + ".nterm"
        default: name
        }
    }
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
    /// The session a `start` made.
    case started(session: String)
    /// What is in `folder`, or at the top of the drive for `nil`.
    case folder(folder: String?, name: String, items: [Item])
    /// The folder a `list` named is not in the drive.
    case noFolder(folder: String?)
    /// Bytes of `file` from byte `from`, and the whole file's `size`.
    case bytes(file: String, from: Int, size: Int, Data)
    /// The file a `read` named can't be read.
    case noFile(file: String)

    private struct Wire: Decodable {
        var type: String
        var file: String?
        var from: Int?
        var size: Int?
        var data: String?
        var folder: String?
        var name: String?
        var items: [Item]?
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
        case ("started", let session?, _): self = .started(session: session)
        case ("folder", _, _):
            self = .folder(folder: wire.folder, name: wire.name ?? "", items: wire.items ?? [])
        case ("no-folder", _, _): self = .noFolder(folder: wire.folder)
        case ("bytes", _, _):
            guard let file = wire.file, let data = Data(base64Encoded: wire.data ?? "") else {
                return nil
            }
            self = .bytes(file: file, from: wire.from ?? 0, size: wire.size ?? 0, data)
        case ("no-file", _, _):
            guard let file = wire.file else { return nil }
            self = .noFile(file: file)
        default: return nil
        }
    }
}
