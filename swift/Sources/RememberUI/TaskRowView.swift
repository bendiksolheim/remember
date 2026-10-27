import SwiftUI
import RememberKit

// `SwiftUI.View`/`SwiftUI.App` are qualified explicitly throughout RememberUI:
// RememberKit's generated `App` and `View` types (mirroring PLAN.md's FFI
// surface) collide by name with those two SwiftUI protocols the moment both
// modules are imported in the same file. There's no Swift toolchain in this
// container to compile-check whether Swift's contextual resolution would
// have sorted out the ambiguity on its own, so every conformance/opaque
// return type is qualified defensively instead of leaving it to chance.

public struct TaskRowView: SwiftUI.View {
    let row: TaskRow
    /// Completes or reopens the row. The caller owns this, since it also
    /// has to start the timer that lets a completed row leave.
    let onToggle: () -> Void
    /// A tap anywhere but the circle: opens the task's details.
    let onOpen: () -> Void

    public init(row: TaskRow, onToggle: @escaping () -> Void, onOpen: @escaping () -> Void) {
        self.row = row
        self.onToggle = onToggle
        self.onOpen = onOpen
    }

    public var body: some SwiftUI.View {
        HStack(spacing: 12) {
            Button(action: onToggle) {
                // The tint (the list's color) highlights the incomplete,
                // actionable state; a done checkmark fades to secondary
                // since it no longer needs attention.
                Image(systemName: row.done ? "checkmark.circle.fill" : "circle")
                    .font(.title3)
                    .foregroundStyle(row.done ? AnyShapeStyle(.secondary) : AnyShapeStyle(.tint))
            }
            .buttonStyle(.plain)
            // A context row is only a heading for its steps.
            .disabled(row.contextOnly)

            Text(row.title)
                .font(.callout)
                .strikethrough(row.done)
                .foregroundStyle(row.done ? .secondary : .primary)

            Spacer(minLength: 0)

            if row.stepCount > 0 {
                Text("\(row.stepsDone)/\(row.stepCount)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            if let label = row.dueLabel {
                DueChip(label: label, tint: dueChipColor(state: row.dueState, done: row.done))
            }
        }
        .padding(.leading, row.parentId == nil ? 0 : 32)
        // The circle is a Button, so it keeps its own taps.
        .contentShape(Rectangle())
        .onTapGesture(perform: onOpen)
        .animation(.easeOut(duration: 0.2), value: row.done)
        // Done rows recede further than just their secondary/strikethrough
        // text color, so they read as clearly separate from active tasks.
        .opacity(row.contextOnly ? 0.4 : (row.done ? 0.55 : 1))
    }
}
