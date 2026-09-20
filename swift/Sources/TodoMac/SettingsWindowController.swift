import AppKit
import SwiftUI
import TodoKit

/// A normal titled window for `SyncSettingsView` — everything else in this
/// target is the borderless `SpotlightPanel`, but sign-in/sync settings
/// don't belong hidden behind a global hotkey, so this is a standard window
/// opened from the status-bar menu instead.
@MainActor
final class SettingsWindowController: NSWindowController {
    convenience init(model: TodoModel) {
        let hosting = NSHostingController(rootView: SyncSettingsView().environment(model))
        let window = NSWindow(contentViewController: hosting)
        window.title = "Todo Sync"
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
