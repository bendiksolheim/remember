import SwiftUI
import TodoKit
import TodoUI

// TodoKit's generated `App`/`View` collide with `SwiftUI.App`/`SwiftUI.View`
// once both modules are imported in the same file — see TaskRowView.swift's
// note in TodoUI. `SwiftUI.App`/`SwiftUI.Scene` are qualified explicitly
// below rather than trusting contextual resolution.

@main
public struct TodoIOSApp: SwiftUI.App {
    // `try!`, matching AppDelegate's own construction of `TodoModel` on
    // Mac: both are app-bootstrap code where a failure here means the app
    // can't run at all, not a recoverable per-request error. No Supabase
    // credentials wired up here yet — sync stays opt-in/disabled on iOS
    // until that's asked for, same as `TodoModel`'s own default.
    @State private var model = try! TodoModel()
    @Environment(\.scenePhase) private var scenePhase

    public init() {}

    public var body: some Scene {
        WindowGroup {
            TaskListView()
                .environment(model)
                .onReceive(
                    NotificationCenter.default.publisher(for: NSNotification.Name.NSSystemTimeZoneDidChange)
                ) { _ in
                    model.setLocalOffsetSeconds()
                }
        }
        // `initial: true` fires this once immediately too, covering launch
        // — mirrors CaptureView's own `onChange(of:initial:)` use for the
        // same "run once now, then again on every real change" shape.
        .onChange(of: scenePhase, initial: true) { _, newPhase in
            if newPhase == .active {
                model.setLocalOffsetSeconds()
            }
        }
    }
}
