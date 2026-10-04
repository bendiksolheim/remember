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
                // Accent highlights the incomplete, actionable state; a
                // done checkmark fades to secondary since it no longer
                // needs attention.
                Image(systemName: row.done ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(row.done ? Color.secondary : Color.accentColor)
            }
            .buttonStyle(.plain)

            VStack(alignment: .leading, spacing: 2) {
                Text(row.title)
                    .strikethrough(row.done)
                    .foregroundStyle(row.done ? .secondary : .primary)
                if let label = row.dueLabel {
                    DueChip(label: label, tint: dueChipColor(state: row.dueState, done: row.done))
                }
            }
        }
        // Done rows recede further than just their secondary/strikethrough
        // text color, so they read as clearly separate from active tasks.
        .opacity(row.done ? 0.55 : 1)
    }
}
