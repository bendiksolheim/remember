import AppKit
import Combine
import ServiceManagement
import SwiftUI

/// The keyboard shortcut, opening at login, and the version. Nothing here
/// touches `TodoModel`: all of it is about this Mac, not about todos.
struct GeneralSettingsView: SwiftUI.View {
    let hotKey: HotKeyStatus

    @State private var loginItem = LoginItem()

    var body: some SwiftUI.View {
        VStack(spacing: 0) {
            Form {
                shortcutSection
                loginItemSection
            }
            .formStyle(.grouped)

            Text(versionText)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .padding(.bottom, 16)
        }
        // The user may have changed it in System Settings since we last
        // looked, or approved a pending registration there.
        .onReceive(NotificationCenter.default.publisher(for: NSWindow.didBecomeKeyNotification)) { _ in
            loginItem.refresh()
        }
    }

    private var shortcutSection: some SwiftUI.View {
        Section {
            LabeledContent("Open Todo") {
                Text(hotKey.shortcut)
                    .font(.body.monospaced())
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 5))
            }
            if !hotKey.isRegistered {
                Label {
                    Text("Another app is already using \(hotKey.shortcut), so the shortcut doesn't open Todo. Quit that app or change its shortcut, then restart Todo.")
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                }
            }
        } header: {
            Text("Keyboard Shortcut")
        } footer: {
            Text("Press it in any app to add a todo or see your list. Press it again, or Esc, to put it away.")
                .foregroundStyle(.secondary)
        }
    }

    private var loginItemSection: some SwiftUI.View {
        Section {
            Toggle("Open Todo at login", isOn: Binding(
                get: { loginItem.isOn },
                set: { loginItem.set($0) }
            ))

            if loginItem.needsApproval {
                VStack(alignment: .leading, spacing: 8) {
                    Text("macOS needs your permission before Todo can open at login. Turn on Todo under Login Items in System Settings.")
                    Button("Open Login Items Settings…") {
                        SMAppService.openSystemSettingsLoginItems()
                    }
                }
            }

            if let error = loginItem.error {
                Text(error)
                    .foregroundStyle(.red)
            }
        } header: {
            Text("Startup")
        } footer: {
            Text("Todo lives in the menu bar. The shortcut only works while Todo is running, so opening it at login means it's always ready.")
                .foregroundStyle(.secondary)
        }
    }

    private var versionText: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String
        // "dev" is what the checked-in Info.plist says; `cargo xtask package`
        // replaces it with the release version.
        guard let version, version != "dev" else { return "Todo (development build)" }
        return "Todo \(version)"
    }
}

/// Opening at login, via `SMAppService.mainApp`. Its status lives in macOS,
/// not in this app, so it is read back after every change rather than
/// tracked here; the user can also change it in System Settings.
@MainActor
@Observable
private final class LoginItem {
    private(set) var status: SMAppService.Status = SMAppService.mainApp.status
    private(set) var error: String?

    /// `requiresApproval` counts as on: Todo is registered, macOS just
    /// hasn't been allowed to act on it yet (see `needsApproval`).
    var isOn: Bool { status == .enabled || status == .requiresApproval }
    var needsApproval: Bool { status == .requiresApproval }

    func refresh() {
        status = SMAppService.mainApp.status
    }

    func set(_ on: Bool) {
        do {
            if on {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            error = nil
        } catch {
            self.error = on
                ? "Couldn't turn on opening at login: \(error.localizedDescription)"
                : "Couldn't turn off opening at login: \(error.localizedDescription)"
        }
        refresh()
    }
}
