import AppKit
import SwiftUI
import TodoKit

/// A normal titled window for settings — everything else in this target is
/// the borderless `SpotlightPanel`, but sign-in/sync and list management
/// don't belong hidden behind a global hotkey, so this is a standard window
/// opened from the status-bar menu instead.
/// Which tab `SettingsRootView` opens on — e.g. the capture panel's "+"
/// pill wants Lists, the status-bar menu item wants Sync.
enum SettingsTab: Hashable {
    case sync
    case lists
}

@MainActor
final class SettingsWindowController: NSWindowController {
    /// `initialTab` only takes effect the moment this controller (and its
    /// `SettingsRootView`) is first created — `AppDelegate` reuses a single
    /// instance across opens, so re-showing an already-open window doesn't
    /// jump it to a different tab, only the very first open picks one.
    convenience init(model: TodoModel, initialTab: SettingsTab = .sync) {
        let hosting = NSHostingController(rootView: SettingsRootView(initialTab: initialTab).environment(model))
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
    @State private var selection: SettingsTab

    init(initialTab: SettingsTab) {
        _selection = State(initialValue: initialTab)
    }

    var body: some SwiftUI.View {
        TabView(selection: $selection) {
            SyncSettingsView()
                .tabItem { Text("Sync") }
                .tag(SettingsTab.sync)
            ListsSettingsView()
                .tabItem { Text("Lists") }
                .tag(SettingsTab.lists)
        }
    }
}
