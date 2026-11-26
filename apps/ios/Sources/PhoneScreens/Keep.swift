import Foundation
import PhoneLink
import Security

/// What this device keeps between runs: its private key, in the keychain, and the computer it
/// paired with.
struct Keep {
    private static let computerName = "computer"
    private static let secretQuery: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: "northroom-phone-link",
        kSecAttrAccount as String: "device-key",
    ]

    var computer: Computer? {
        get {
            UserDefaults.standard.data(forKey: Keep.computerName).flatMap {
                try? JSONDecoder().decode(Computer.self, from: $0)
            }
        }
        nonmutating set {
            let data = newValue.flatMap { try? JSONEncoder().encode($0) }
            UserDefaults.standard.set(data, forKey: Keep.computerName)
        }
    }

    /// This device's private key: the one kept, or a new one, kept for next time.
    func secret() -> Data {
        var query = Keep.secretQuery
        query[kSecReturnData as String] = true
        var found: CFTypeRef?
        if SecItemCopyMatching(query as CFDictionary, &found) == errSecSuccess,
            let kept = found as? Data, kept.count == 32
        {
            return kept
        }
        let fresh = Link.newSecret()
        var item = Keep.secretQuery
        item[kSecValueData as String] = fresh
        // Never leaves this device, and is there for a reconnect before the first unlock.
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        SecItemDelete(Keep.secretQuery as CFDictionary)
        SecItemAdd(item as CFDictionary, nil)
        return fresh
    }

    /// Forgets the computer and this device's key, so a later pairing is as a new device.
    func forget() {
        computer = nil
        SecItemDelete(Keep.secretQuery as CFDictionary)
    }
}
