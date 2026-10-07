import Foundation
import Security

/// A Keychain call returned something other than success.
struct KeychainError: Error, CustomStringConvertible {
    let status: OSStatus

    var description: String {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "unknown error"
        return "Keychain error \(status): \(message)"
    }
}

/// Thin, logic-free wrapper around the platform Keychain for one opaque
/// session blob. This is the one deliberate exception to "no business logic
/// in Swift": secure token storage needs the OS Keychain, which Rust has no
/// reasonable cross-platform way to reach, but there is no decision made
/// here — just get/set/delete for a single fixed (service, account) pair.
///
/// macOS still uses the legacy file-based keychain: the data protection
/// keychain needs a keychain-access-groups entitlement, which an ad-hoc
/// signed build can't carry. Switching (with a migration of the existing
/// item) waits on Developer ID signing — security report findings 3 and 9.
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

    /// Updates the item in place, adding it only if there is none. Never
    /// deletes first: refresh tokens rotate, so a delete followed by a
    /// failed add would lose the session for good.
    static func save(_ data: Data) throws {
        var attributes: [String: Any] = [kSecValueData as String: data]
        #if os(iOS)
        // Readable by background sync while the phone is locked, and never
        // restored onto another device from a backup. Set on update too, so
        // an item stored before this moves over. The legacy macOS keychain
        // has no such classes.
        attributes[kSecAttrAccessible as String] =
            kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        #endif

        let status = SecItemUpdate(baseQuery as CFDictionary, attributes as CFDictionary)
        if status == errSecSuccess { return }
        guard status == errSecItemNotFound else { throw KeychainError(status: status) }

        let addStatus = SecItemAdd(
            baseQuery.merging(attributes) { _, new in new } as CFDictionary, nil)
        guard addStatus == errSecSuccess else { throw KeychainError(status: addStatus) }
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

    /// Succeeds if there was nothing to delete.
    static func delete() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
    }
}
