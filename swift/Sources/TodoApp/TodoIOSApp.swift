import SwiftUI
import TodoUI

@main
public struct TodoIOSApp: App {
    public init() {}
    public var body: some Scene {
        WindowGroup { TaskListView() }
    }
}
