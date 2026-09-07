import SwiftUI
import TodoKit

// `SwiftUI.View`/`SwiftUI.App` are qualified explicitly throughout TodoUI:
// TodoKit's generated `App` and `View` types (mirroring PLAN.md's FFI
// surface) collide by name with those two SwiftUI protocols the moment both
// modules are imported in the same file. There's no Swift toolchain in this
// container to compile-check whether Swift's contextual resolution would
// have sorted out the ambiguity on its own, so every conformance/opaque
// return type is qualified defensively instead of leaving it to chance.

public struct TaskRowView: SwiftUI.View {
    let row: TaskRow

    @Environment(TodoModel.self) private var model

    public init(row: TaskRow) {
        self.row = row
    }

    public var body: some SwiftUI.View {
        HStack {
            Button {
                model.dispatch(.setDone(id: row.id, done: !row.done))
            } label: {
                Image(systemName: row.done ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(row.done ? Color.accentColor : Color.secondary)
            }
            .buttonStyle(.plain)

            VStack(alignment: .leading, spacing: 2) {
                Text(row.title)
                    .strikethrough(row.done)
                    .foregroundStyle(row.done ? .secondary : .primary)
                if let label = row.dueLabel {
                    Text(label)
                        .font(.caption)
                        .foregroundStyle(row.overdue ? Color.red : Color.secondary)
                }
            }
        }
    }
}
