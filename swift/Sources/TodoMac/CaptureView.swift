import SwiftUI
import TodoKit

// Same qualification convention as TodoUI (see TaskRowView.swift): TodoKit's
// generated `App`/`View` collide by name with `SwiftUI.App`/`SwiftUI.View`
// once both modules are imported, so conformances/opaque returns are
// qualified explicitly rather than trusting contextual resolution --
// unverified since there's no compiler in this container.

/// The entire macOS UI for v1: a quick-add field plus the live incomplete
/// task list, hosted inside `SpotlightPanel`. Editing, deleting, and
/// reordering are out of scope here -- those remain reachable only via
/// `TodoUI`'s `TaskListView`, which iOS still uses but this target no
/// longer wires up.
struct CaptureView: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    var onDismiss: () -> Void
    /// Called with the panel's required total height whenever the row
    /// count changes, so `SpotlightPanel` can resize the actual window --
    /// a plain SwiftUI `ScrollView` has no natural "hug my content up to a
    /// max, then scroll" sizing behavior, so height is computed here from
    /// `displayRows.count` instead of trusting layout to report it.
    var onContentHeightChange: (CGFloat) -> Void

    @State private var input = ""
    @FocusState private var inputFocused: Bool
    /// Rows that were just checked off, held here (not in `model.snapshot`,
    /// which already excludes them once `View.active` is set) so they can
    /// visibly linger for ~3s instead of vanishing the instant they're
    /// marked done.
    @State private var ghosts: [TaskRow] = []

    var body: some SwiftUI.View {
        VStack(spacing: 0) {
            TextField("Add a task…", text: $input)
                .textFieldStyle(.plain)
                .font(.system(size: 22))
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
                .focused($inputFocused)
                .onSubmit(addTask)

            if !displayRows.isEmpty {
                Divider()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(displayRows, id: \.row.id) { entry in
                            CaptureTaskRow(row: entry.row, completed: entry.completed) {
                                complete(entry.row)
                            }
                        }
                    }
                }
                // Deliberately an exact height, not `maxHeight`: a bare
                // ScrollView doesn't hug shorter content, it greedily takes
                // whatever space it's offered, so an exact height computed
                // from the row count is what makes "small list -> small
                // panel, long list -> capped + scrolls" actually happen.
                .frame(height: rowsAreaHeight)
            }
        }
        .frame(width: PanelMetrics.width)
        .onExitCommand(perform: onDismiss)
        .onAppear { inputFocused = true }
        .onChange(of: contentHeight, initial: true) { _, newValue in
            onContentHeightChange(newValue)
        }
        .overlay {
            // Hidden buttons rather than a raw NSEvent monitor: SwiftUI's
            // `.keyboardShortcut` is honored by NSHostingView's own
            // performKeyEquivalent handling even with no app-level
            // CommandGroup, so this Just Works inside the accessory-app
            // panel without extra AppKit plumbing.
            Group {
                Button("Undo") { model.dispatch(.undo) }
                    .keyboardShortcut("z", modifiers: .command)
                Button("Redo") { model.dispatch(.redo) }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
            }
            .hidden()
        }
    }

    private var displayRows: [(row: TaskRow, completed: Bool)] {
        let ghostIDs = Set(ghosts.map(\.id))
        let active = (model.snapshot?.rows ?? []).filter { !ghostIDs.contains($0.id) }
        return ghosts.map { ($0, true) } + active.map { ($0, false) }
    }

    private var rowsAreaHeight: CGFloat {
        CGFloat(min(displayRows.count, PanelMetrics.maxVisibleRows)) * PanelMetrics.rowHeight
    }

    private var contentHeight: CGFloat {
        PanelMetrics.inputHeight + (displayRows.isEmpty ? 0 : rowsAreaHeight)
    }

    private func addTask() {
        let title = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { return }
        model.dispatch(.add(title: title, after: nil))
        input = ""
    }

    private func complete(_ row: TaskRow) {
        guard !ghosts.contains(where: { $0.id == row.id }) else { return }
        model.dispatch(.setDone(id: row.id, done: true))
        ghosts.append(row)
        let id = row.id
        Task {
            try? await Task.sleep(for: .seconds(3))
            ghosts.removeAll { $0.id == id }
        }
    }
}

private struct CaptureTaskRow: SwiftUI.View {
    let row: TaskRow
    let completed: Bool
    let onToggle: () -> Void

    var body: some SwiftUI.View {
        HStack(spacing: 10) {
            Button(action: onToggle) {
                Image(systemName: completed ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(completed ? Color.secondary : Color.accentColor)
            }
            .buttonStyle(.plain)
            .disabled(completed)

            Text(row.title)
                .strikethrough(completed)
                .foregroundStyle(completed ? .secondary : .primary)

            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .animation(.easeOut(duration: 0.2), value: completed)
    }
}
