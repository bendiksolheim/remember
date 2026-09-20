import Cocoa
import ServiceManagement
import SwiftUI
import TodoKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, SpotlightPanelDelegate {
    private var model: TodoModel!
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

        // Bare project URL, no path — `todo-sync`'s `HttpTransport`/`AuthClient`
        // append `/rest/v1/...` and `/auth/v1/...` themselves.
        model = try! TodoModel(
            supabaseURL: "https://xzmbkeeaycyaluadzxhe.supabase.co",
            supabaseAnonKey: "sb_publishable_xRie_YdgGu7lS0ozQzYeUg_Ww0Ek7mE"
        )
        // Set once here rather than relying on CaptureView's onAppear:
        // the panel's SwiftUI content exists (and can start reacting to
        // model changes) well before it's ever ordered on screen, so
        // there's no guarantee onAppear fires before the user's first
        // hotkey press.
        model.setView(.active)

        panel = SpotlightPanel(content: CaptureView(
            onDismiss: { [weak self] in self?.hide() },
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
            NSLog("Todo: failed to register the global hotkey (⌥⌘Space may already be claimed by another app).")
        }

        setUpStatusItem()
        registerLoginItemIfNeeded()
        observeSyncTriggers()
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
        item.button?.image = NSImage(systemSymbolName: "checklist", accessibilityDescription: "Todo")

        let menu = NSMenu()
        let syncItem = NSMenuItem(title: "Sync…", action: #selector(showSyncSettings), keyEquivalent: ",")
        syncItem.target = self
        menu.addItem(syncItem)
        menu.addItem(.separator())
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        item.menu = menu

        statusItem = item
    }

    @objc private func showSyncSettings() {
        if settingsWindowController == nil {
            settingsWindowController = SettingsWindowController(model: model)
        }
        settingsWindowController?.show()
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

    // MARK: - Login item

    private func registerLoginItemIfNeeded() {
        guard SMAppService.mainApp.status != .enabled else { return }
        do {
            try SMAppService.mainApp.register()
        } catch {
            NSLog("Todo: failed to register as a login item: \(error)")
        }
    }
}
