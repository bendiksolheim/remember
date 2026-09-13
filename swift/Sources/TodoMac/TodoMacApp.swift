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
    }
}
