import Foundation

/// What a pairing link says (`northroom://pair?…`, docs/phone-link.md): where the computer is
/// and the code that pairs this device with it.
public struct Invite: Equatable, Sendable {
    /// The computer's public key: 64 hex digits.
    public var key: String
    public var code: String
    /// The computer's name.
    public var host: String
    public var relay: String?
    /// The computer's addresses on its own networks, as `ip:port`.
    public var direct: [String]

    /// Reads a pairing link. `nil` when the text is not one, or lacks the key or the code.
    public init?(link: String) {
        let start = "northroom://pair?"
        guard link.hasPrefix(start) else { return nil }
        var values: [(name: String, value: String)] = []
        for pair in link.dropFirst(start.count).split(separator: "&") {
            let parts = pair.split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false)
            guard parts.count == 2, let value = String(parts[1]).removingPercentEncoding else {
                return nil
            }
            values.append((String(parts[0]), value))
        }
        func first(_ name: String) -> String? { values.first { $0.name == name }?.value }
        guard let key = first("key"), Invite.isKey(key), let code = first("code"), !code.isEmpty
        else { return nil }
        self.key = key
        self.code = code
        self.host = first("name") ?? ""
        self.relay = first("relay")
        self.direct = values.filter { $0.name == "at" }.map(\.value)
    }

    static func isKey(_ text: String) -> Bool {
        text.utf8.count == 64 && text.utf8.allSatisfy { Character(UnicodeScalar($0)).isHexDigit }
    }
}

/// A computer this device has paired with: enough to dial it again without a code.
public struct Computer: Codable, Equatable, Sendable {
    public var key: String
    public var name: String
    public var relay: String?
    public var direct: [String]

    public init(invite: Invite) {
        key = invite.key
        name = invite.host
        relay = invite.relay
        direct = invite.direct
    }
}

extension Data {
    /// Bytes from an even number of hex digits.
    init?(hex: String) {
        let digits = Array(hex.utf8)
        guard digits.count.isMultiple(of: 2) else { return nil }
        var bytes = [UInt8]()
        for index in stride(from: 0, to: digits.count, by: 2) {
            guard let byte = UInt8(String(decoding: digits[index...index + 1], as: UTF8.self), radix: 16)
            else { return nil }
            bytes.append(byte)
        }
        self.init(bytes)
    }
}
