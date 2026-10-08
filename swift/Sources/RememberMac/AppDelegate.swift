import Cocoa
import SwiftUI
import RememberKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, SpotlightPanelDelegate {
    private var model: RememberModel!
    private var panel: SpotlightPanel!
    private var hotKey: GlobalHotKey?
    private var statusItem: NSStatusItem?
    private var settingsWindowController: SettingsWindowController?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let bundleID = Bundle.main.bundleIdentifier ?? ""
        let peers = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
        if peers.count > 1 {
            // Another instance already owns the hotkey; don't fight it.
            NSApp.terminate(nil)
            return
        }

        NSApp.setActivationPolicy(.accessory)

        // Bare project URL, no path — `remember-sync`'s `HttpTransport`/`AuthClient`
        // append `/rest/v1/...` and `/auth/v1/...` themselves.
        model = try! RememberModel(
            supabaseURL: "https://xzmbkeeaycyaluadzxhe.supabase.co",
            supabaseAnonKey: "sb_publishable_xRie_YdgGu7lS0ozQzYeUg_Ww0Ek7mE"
        )
        // Set once here rather than relying on CaptureView's onAppear:
        // the panel's SwiftUI content exists (and can start reacting to
        // model changes) well before it's ever ordered on screen, so
        // there's no guarantee onAppear fires before the user's first
        // hotkey press.
        model.setView(.active)
        model.setLocalOffsetSeconds()

        panel = SpotlightPanel(content: CaptureView(
            onDismiss: { [weak self] in self?.hide() },
            onOpenListsSettings: { [weak self] in self?.showSettings(tab: .lists, focusNewList: true) },
            onContentHeightChange: { [weak self] height in self?.panel.resize(toContentHeight: height) }
        ).environment(model))
        panel.spotlightDelegate = self

        hotKey = GlobalHotKey(
            keyCode: CarbonKeyCode.space,
            modifiers: CarbonModifier.option | CarbonModifier.command,
            // Carbon's event callback is a plain C function pointer, so it
            // can't be proven MainActor-isolated at compile time even
            // though it always fires on the main run loop -- hopping
            // through a Task keeps this correct under strict concurrency
            // checking regardless.
            onPress: { [weak self] in
                Task { @MainActor in self?.toggle() }
            }
        )
        if hotKey == nil {
            NSLog("Remember: failed to register the global hotkey (⌥⌘Space may already be claimed by another app).")
        }

        setUpStatusItem()
        observeSyncTriggers()
        observeLocalOffsetTriggers()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // The panel is a manually-managed NSPanel, not a Scene-owned
        // window; ordering it out must never be read as "last window
        // closed, quit the app" for a background/accessory app.
        false
    }

    // MARK: - Show/hide

    private func toggle() {
        if panel.isVisible {
            hide()
        } else {
            show()
        }
    }

    private func show() {
        panel.positionOnActiveScreen()
        // No NSApp.activate here: `.nonactivatingPanel` + the canBecomeKey
        // override let the panel take keyboard focus without activating us
        // or deactivating whatever app was previously frontmost, so there's
        // nothing to restore when the panel goes away again.
        panel.makeKeyAndOrderFront(nil)
        // The closest thing this app has to "came to the foreground" --
        // it's a hotkey-driven accessory app with no normal window, so
        // `NSApplication.didBecomeActiveNotification` (handled separately,
        // for the settings window) rarely fires from this interaction path
        // at all. The user just showed up; don't make them wait for the
        // periodic fallback to find out what changed elsewhere.
        model.syncSoon()
    }

    private func hide() {
        panel.orderOut(nil)
    }

    func spotlightPanelDidResignKey(_ panel: SpotlightPanel) {
        hide()
    }

    // MARK: - Menu bar

    private func setUpStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "checklist", accessibilityDescription: "Remember")

        let menu = NSMenu()
        let settingsItem = NSMenuItem(title: "Settings…", action: #selector(openSettings), keyEquivalent: ",")
        settingsItem.target = self
        menu.addItem(settingsItem)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        item.menu = menu

        statusItem = item
    }

    @objc private func openSettings() {
        showSettings()
    }

    /// Also the app menu's "Settings…" (see `RememberMacApp`). `tab` nil opens
    /// whichever tab the window last showed.
    func showSettings(tab: SettingsTab? = nil, focusNewList: Bool = false) {
        if settingsWindowController == nil {
            settingsWindowController = SettingsWindowController(
                model: model,
                hotKey: HotKeyStatus(shortcut: "⌥⌘Space", isRegistered: hotKey != nil),
                onClose: {
                    // Back to a menu-bar-only app once Settings is gone.
                    NSApp.setActivationPolicy(.accessory)
                }
            )
        }
        // In the Dock and ⌘Tab while Settings is open, so it can be found
        // again after another app's window covers it.
        NSApp.setActivationPolicy(.regular)
        settingsWindowController?.show(tab: tab, focusNewList: focusNewList)
    }

    // MARK: - Sync triggers

    /// Two platform lifecycle events worth an immediate sync attempt
    /// beyond the debounce-after-edit/periodic-fallback the coordinator
    /// already runs on its own: the Mac waking from sleep (the most likely
    /// moment for a long stretch of missed changes from other devices to
    /// have piled up), and this app itself becoming active (covers the
    /// settings window activating it -- `show()` above covers the far more
    /// common hotkey-driven path separately, since that one deliberately
    /// never activates the app).
    private func observeSyncTriggers() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            Task { @MainActor in self?.model.syncSoon() }
        }

        NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            Task { @MainActor in self?.model.syncSoon() }
        }
    }

    // MARK: - Local offset

    /// Keeps the due-date day math (`detectDue`, every row's overdue/label)
    /// aligned with the device's actual local timezone. Refreshed on the
    /// same wake/active triggers `observeSyncTriggers` already watches —
    /// both are "something about the world outside this app may have
    /// changed" moments — plus the system's own timezone-change
    /// notification, for a travelling Mac that never sleeps in between.
    private func observeLocalOffsetTriggers() {
        NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            Task { @MainActor in self?.model.setLocalOffsetSeconds() }
        }

        NotificationCenter.default.addObserver(
            forName: NSApplication.didBecomeActiveNotification,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            Task { @MainActor in self?.model.setLocalOffsetSeconds() }
        }

        NotificationCenter.default.addObserver(
            forName: NSNotification.Name.NSSystemTimeZoneDidChange,
            object: nil,
            queue: nil
        ) { [weak self] _ in
            Task { @MainActor in self?.model.setLocalOffsetSeconds() }
        }
    }
}
