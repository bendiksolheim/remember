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

    func applicationDidFinishLaunching(_ notification: Notification) {
        let bundleID = Bundle.main.bundleIdentifier ?? ""
        let peers = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
        if peers.count > 1 {
            // Another instance already owns the hotkey; don't fight it.
            NSApp.terminate(nil)
            return
        }

        NSApp.setActivationPolicy(.accessory)

        model = try! TodoModel()
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
    }

    private func hide() {
        panel.orderOut(nil)
    }

    func spotlightPanelDidResignKey(_ panel: SpotlightPanel) {
        hide()
    }

    // MARK: - Menu bar (Quit only for v1 -- see plan's "menu bar contents" decision)

    private func setUpStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        item.button?.image = NSImage(systemSymbolName: "checklist", accessibilityDescription: "Todo")

        let menu = NSMenu()
        menu.addItem(NSMenuItem(title: "Quit", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"))
        item.menu = menu

        statusItem = item
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
