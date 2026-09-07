import SwiftUI
import TodoKit
import TodoUI

@main
struct TodoMacApp: SwiftUI.App {
    @State private var model = try! TodoModel()

    var body: some Scene {
        WindowGroup {
            NavigationStack {
                TaskListView()
            }
            .environment(model)
        }
        .defaultSize(width: 420, height: 640)
    }
}
