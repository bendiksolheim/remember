import AppKit
import SwiftUI
import TodoKit

/// A normal titled window for settings — everything else in this target is
/// the borderless `SpotlightPanel`, but sign-in/sync and list management
/// don't belong hidden behind a global hotkey, so this is a standard window
/// opened from the status-bar menu instead.
@MainActor
final class SettingsWindowController: NSWindowController {
    convenience init(model: TodoModel) {
        let hosting = NSHostingController(rootView: SettingsRootView().environment(model))
        let window = NSWindow(contentViewController: hosting)
        window.title = "Todo Settings"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        self.init(window: window)
    }

    /// The app is `.accessory` (no Dock icon, doesn't auto-activate) — this
    /// window needs an explicit `activate` or it can open behind whatever
    /// app currently has focus.
    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
    }
}

/// Tabs between sync settings and list management -- the two things this
/// window exists for.
private struct SettingsRootView: SwiftUI.View {
    var body: some SwiftUI.View {
        TabView {
            SyncSettingsView()
                .tabItem { Text("Sync") }
            ListsSettingsView()
                .tabItem { Text("Lists") }
        }
    }
}
