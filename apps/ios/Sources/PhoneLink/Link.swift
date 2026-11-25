import Foundation
import IrohLib

/// Why a link could not be made or ended.
public enum LinkError: Error, Equatable {
    case badAddress
    case refused(why: String)
    case closed
}

/// One open link to a computer: messages each way, each sent as its length (four bytes,
/// big-endian) and then its bytes.
public actor Link {
    /// The protocol both sides name to connect.
    static let alpn = Data("northroom/1".utf8)
    /// The longest message either side accepts.
    static let mostMessageBytes: UInt32 = 4 * 1024 * 1024

    private let endpoint: Endpoint
    private let connection: Connection
    private let send: SendStream
    private let receive: RecvStream

    /// Dials `computer` as the device whose private key is `secret` (32 bytes).
    public init(secret: Data, computer: Computer) async throws {
        guard let key = Data(hex: computer.key), key.count == 32 else { throw LinkError.badAddress }
        let builder = EndpointBuilder()
        // No address is published or looked up: the pairing link gave the computer's.
        builder.applyMinimal()
        try builder.secretKey(bytes: secret)
        if let relay = computer.relay {
            builder.relayMode(mode: try RelayMode.customFromUrls(urls: [relay]))
        } else {
            builder.relayMode(mode: RelayMode.disabled())
        }
        endpoint = try await builder.bind()
        let address = EndpointAddr(
            id: try EndpointId.fromBytes(bytes: key),
            relayUrl: computer.relay,
            addresses: computer.direct
        )
        connection = try await endpoint.connect(addr: address, alpn: Link.alpn)
        let stream = try await connection.openBi()
        send = stream.send()
        receive = stream.recv()
    }

    /// A new private key for this device, to keep and reuse.
    public static func newSecret() -> Data {
        SecretKey.generate().toBytes()
    }

    public func ask(_ ask: Ask) async throws {
        let bytes = ask.encoded()
        var length = UInt32(bytes.count).bigEndian
        try await send.writeAll(buf: Data(bytes: &length, count: 4) + bytes)
    }

    /// The computer's next message this version knows. Throws `LinkError.closed` when the link
    /// has ended.
    public func next() async throws -> Tell {
        while true {
            guard let header = try? await receive.readExact(size: 4), header.count == 4 else {
                throw LinkError.closed
            }
            let length = header.reduce(UInt32(0)) { $0 << 8 | UInt32($1) }
            guard length <= Link.mostMessageBytes,
                let bytes = try? await receive.readExact(size: length)
            else { throw LinkError.closed }
            if let tell = Tell(bytes) {
                return tell
            }
        }
    }

    /// Pairs with the code from the pairing link, or as an already paired device says hello.
    /// Returns the computer's name. Throws `LinkError.refused` when turned away.
    public func greet(code: String?, deviceName: String) async throws -> String {
        if let code {
            try await ask(.pair(code: code, name: deviceName))
            _ = try await welcomed()
        }
        try await ask(.hello)
        return try await welcomed()
    }

    private func welcomed() async throws -> String {
        switch try await next() {
        case .welcome(let host): return host
        case .refused(let why): throw LinkError.refused(why: why)
        default: throw LinkError.closed
        }
    }

    public func close() async {
        try? await send.finish()
        try? connection.close(errorCode: 0, reason: Data("closed".utf8))
        try? await endpoint.close()
    }
}
