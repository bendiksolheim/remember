import SwiftUI

// All lifecycle -- the hotkey, the panel, showing/hiding, the menu-bar item
// -- lives in AppDelegate now; TaskListView/TaskDetailView (TodoUI) are no
// longer wired up here (only iOS's TodoIOSApp still uses them). `Settings`
// is the one SwiftUI Scene that creates no visible window, since the App
// protocol requires at least one Scene but this app's real UI is the
// manually-managed SpotlightPanel, not anything Scene-owned.
@main
struct TodoMacApp: SwiftUI.App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        Settings {
            EmptyView()
        }
        // The app menu (visible while the Settings window makes the app
        // `.regular`) would otherwise open this scene's empty window.
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button("Settings…") { appDelegate.showSettings() }
                    .keyboardShortcut(",")
            }
        }
    }
}
