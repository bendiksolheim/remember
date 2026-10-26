import SwiftUI
import UIKit
import RememberKit
import RememberUI

// RememberKit's generated `App`/`View` collide with `SwiftUI.App`/`SwiftUI.View`
// once both modules are imported in the same file — see TaskRowView.swift's
// note in RememberUI. `SwiftUI.App`/`SwiftUI.Scene` are qualified explicitly
// below rather than trusting contextual resolution.

@main
public struct RememberIOSApp: SwiftUI.App {
    @State private var model = Self.makeModel()
    @State private var backgroundPush = BackgroundPush()
    @Environment(\.scenePhase) private var scenePhase

    public init() {}

    /// `try!`, matching AppDelegate's own construction of `RememberModel` on
    /// Mac: both are app-bootstrap code where a failure here means the app
    /// can't run at all, not a recoverable per-request error. Active-only,
    /// like the Mac panel.
    private static func makeModel() -> RememberModel {
        let model = try! RememberModel(
            supabaseURL: SyncBackend.url,
            supabaseAnonKey: SyncBackend.anonKey
        )
        model.setView(.active)
        return model
    }

    public var body: some Scene {
        WindowGroup {
            NavigationStack {
                TaskListView()
            }
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
            switch newPhase {
            case .active:
                // Same "the user just showed up" moments the Mac reacts to
                // on wake and on showing the panel.
                model.setLocalOffsetSeconds()
                model.syncSoon()
            case .background:
                // Coming back starts from a settled list, as the Mac panel
                // does after hiding.
                model.releaseHeld()
                model.flush()
                backgroundPush.start(model: model)
            default:
                break
            }
        }
    }
}

/// Keeps the app running in the background just long enough for
/// `RememberModel.pushPendingChanges` to finish — iOS suspends it within
/// seconds otherwise, and a task added right before leaving wouldn't reach
/// the other devices until the next launch. iOS-only, so it lives here.
@MainActor
private final class BackgroundPush {
    private var task: UIBackgroundTaskIdentifier = .invalid

    func start(model: RememberModel) {
        end()
        task = UIApplication.shared.beginBackgroundTask(withName: "Push changes") { [weak self] in
            // Out of time: iOS calls this on the main thread.
            MainActor.assumeIsolated { self?.end() }
        }
        model.pushPendingChanges { [weak self] in self?.end() }
    }

    private func end() {
        guard task != .invalid else { return }
        UIApplication.shared.endBackgroundTask(task)
        task = .invalid
    }
}
