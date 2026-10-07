import AppKit
import SwiftUI
import TodoKit

/// The Settings window's tabs, in toolbar order. `rawValue` is the tab's
/// index in `SettingsWindowController`'s `NSTabViewController`.
enum SettingsTab: Int {
    case general
    case lists
    case account
}

/// Every tab gets the same size, so switching tabs never resizes the window
/// (a tab that needs more room scrolls instead).
enum SettingsMetrics {
    static let size = NSSize(width: 520, height: 460)
}

/// Whether the global hotkey could be registered at launch, for the General
/// tab to show -- a failed registration is otherwise invisible to the user.
struct HotKeyStatus {
    let shortcut: String
    let isRegistered: Bool
}

/// Requests from outside the window to the tab views inside it.
@MainActor
@Observable
final class SettingsNavigation {
    /// Set when the capture panel's "+" pill opens Settings: the Lists tab
    /// puts the cursor in its "New list" field, then clears this.
    var focusNewList = false
}

/// A normal titled window with toolbar tabs (General, Lists, Account), the
/// standard macOS settings layout. Everything else in this target is the
/// borderless `SpotlightPanel`; settings are opened from the status-bar
/// menu, the app menu's "Settings…" (⌘,), or the panel's "+" pill.
///
/// The app is `.accessory` (no Dock icon, not in ⌘Tab), so a window that
/// falls behind another app's windows could only be brought back from the
/// menu bar. `AppDelegate` therefore makes the app `.regular` while this
/// window is open and switches back in `onClose`.
@MainActor
final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    private let tabs: NSTabViewController
    private let navigation: SettingsNavigation
    private let onClose: () -> Void

    init(model: TodoModel, hotKey: HotKeyStatus, onClose: @escaping () -> Void) {
        self.onClose = onClose
        let navigation = SettingsNavigation()
        self.navigation = navigation

        let tabs = NSTabViewController()
        tabs.tabStyle = .toolbar
        // The window takes its title from this controller. By default that
        // is the selected tab's view controller's title, and the hosting
        // controllers have none, so the window said "Untitled".
        tabs.canPropagateSelectedChildViewControllerTitle = false
        let appName = Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "Todo"
        tabs.title = "\(appName) Settings"
        tabs.addTabViewItem(SettingsWindowController.tab(
            GeneralSettingsView(hotKey: hotKey),
            label: "General", symbol: "gearshape", model: model, navigation: navigation
        ))
        tabs.addTabViewItem(SettingsWindowController.tab(
            ListsSettingsView(),
            label: "Lists", symbol: "list.bullet", model: model, navigation: navigation
        ))
        tabs.addTabViewItem(SettingsWindowController.tab(
            AccountSettingsView(),
            label: "Account", symbol: "person.crop.circle", model: model, navigation: navigation
        ))
        self.tabs = tabs

        let window = SettingsWindow(contentViewController: tabs)
        window.styleMask = [.titled, .closable]
        window.toolbarStyle = .preference
        window.title = "\(appName) Settings"
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.delegate = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    private static func tab(
        _ view: some SwiftUI.View,
        label: String,
        symbol: String,
        model: TodoModel,
        navigation: SettingsNavigation
    ) -> NSTabViewItem {
        let hosting = NSHostingController(rootView: view
            .frame(width: SettingsMetrics.size.width, height: SettingsMetrics.size.height)
            .environment(model)
            .environment(navigation))
        // A fixed size, not one SwiftUI derives from the content: that is
        // what made the old window grow on Lists and never shrink back.
        hosting.sizingOptions = []
        hosting.preferredContentSize = SettingsMetrics.size

        let item = NSTabViewItem(viewController: hosting)
        item.label = label
        item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
        return item
    }

    /// Brings the window up, on `tab` if given; otherwise on whichever tab
    /// it was last showing (General the first time).
    func show(tab: SettingsTab? = nil, focusNewList: Bool = false) {
        if let tab {
            tabs.selectedTabViewItemIndex = tab.rawValue
        }
        if focusNewList {
            navigation.focusNewList = true
        }
        // Centered only when it opens, never yanked back to the middle when
        // it's already on screen somewhere the user put it.
        if window?.isVisible != true {
            window?.center()
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        onClose()
    }
}

/// ⌘W closes the window even if the app's main menu has no Close item
/// (it has none of its own while the app is `.accessory`).
private final class SettingsWindow: NSWindow {
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        if event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
           event.charactersIgnoringModifiers == "w" {
            performClose(nil)
            return true
        }
        return super.performKeyEquivalent(with: event)
    }
}
