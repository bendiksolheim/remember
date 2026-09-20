import Foundation
import Security

/// Thin, logic-free wrapper around the platform Keychain for one opaque
/// session blob. This is the one deliberate exception to "no business logic
/// in Swift": secure token storage needs the OS Keychain, which Rust has no
/// reasonable cross-platform way to reach, but there is no decision made
/// here — just get/set/delete for a single fixed (service, account) pair.
enum KeychainStore {
    private static let service = "no.bendik.todo.sync"
    private static let account = "session"

    private static var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    static func save(_ data: Data) {
        SecItemDelete(baseQuery as CFDictionary)
        var attributes = baseQuery
        attributes[kSecValueData as String] = data
        SecItemAdd(attributes as CFDictionary, nil)
    }

    static func load() -> Data? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess else { return nil }
        return result as? Data
    }

    static func delete() {
        SecItemDelete(baseQuery as CFDictionary)
    }
}
