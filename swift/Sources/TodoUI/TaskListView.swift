import SwiftUI
import TodoKit

public struct TaskListView: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    @State private var newTitle = ""
    /// Live result of `TodoModel.detectDue` for `newTitle`, re-checked on
    /// every keystroke. `nil` means no recognized trailing phrase.
    @State private var dueDetection: DueDetection?
    /// Set when the user taps the badge to reject a detection for this
    /// input — cleared again on the next keystroke, so a fresh phrase is
    /// never silently suppressed by an old dismissal.
    @State private var dueDismissed = false

    public init() {}

    public var body: some SwiftUI.View {
        List {
            Section {
                HStack {
                    TextField("New task", text: $newTitle)
                        .onSubmit(addTask)
                        .onChange(of: newTitle) { _, newValue in
                            dueDetection = newValue.isEmpty ? nil : model.detectDue(newValue)
                            dueDismissed = false
                        }
                    if let detection = dueDetection, !dueDismissed {
                        DueBadge(label: detection.label) { dueDismissed = true }
                    }
                    Button("Add", action: addTask)
                        .disabled(newTitle.isEmpty)
                }
            }
            Section {
                ForEach(model.snapshot?.rows ?? [], id: \.id) { row in
                    NavigationLink(value: row.id) {
                        TaskRowView(row: row)
                    }
                }
                .onDelete(perform: delete)
                .onMove(perform: move)
            }
        }
        .navigationDestination(for: String.self) { id in
            if let row = model.snapshot?.rows.first(where: { $0.id == id }) {
                TaskDetailView(row: row)
            }
        }
        .navigationTitle("Todo")
        .toolbar {
            ToolbarItem {
                Picker("Filter", selection: viewBinding) {
                    Text("All").tag(TodoKit.View.all)
                    Text("Active").tag(TodoKit.View.active)
                    Text("Completed").tag(TodoKit.View.completed)
                }
                .pickerStyle(.segmented)
            }
            ToolbarItem {
                Button("Undo", systemImage: "arrow.uturn.backward") {
                    model.dispatch(.undo)
                }
                .disabled(!(model.snapshot?.canUndo ?? false))
            }
            ToolbarItem {
                Button("Redo", systemImage: "arrow.uturn.forward") {
                    model.dispatch(.redo)
                }
                .disabled(!(model.snapshot?.canRedo ?? false))
            }
        }
    }

    private var viewBinding: Binding<TodoKit.View> {
        Binding(
            get: { model.snapshot?.view ?? .all },
            set: { model.setView($0) }
        )
    }

    private func addTask() {
        if let detection = dueDetection, !dueDismissed {
            model.dispatch(.add(title: detection.strippedTitle, after: nil, due: detection.due))
        } else {
            model.dispatch(.add(title: newTitle, after: nil, due: nil))
        }
        newTitle = ""
        dueDetection = nil
        dueDismissed = false
    }

    private func delete(at offsets: IndexSet) {
        guard let rows = model.snapshot?.rows else { return }
        for index in offsets {
            model.dispatch(.delete(id: rows[index].id))
        }
    }

    /// `destination` follows the same convention as
    /// `Array.move(fromOffsets:toOffset:)`: a post-removal insertion index.
    /// No local reordered array is kept — this just resolves `destination`
    /// to the id it should land after and dispatches, same as every other
    /// mutation here; the next snapshot redraws the list.
    private func move(from source: IndexSet, to destination: Int) {
        guard let rows = model.snapshot?.rows, let sourceIndex = source.first else { return }
        let movingID = rows[sourceIndex].id
        let remaining = rows.enumerated()
            .filter { $0.offset != sourceIndex }
            .map(\.element)
        let afterID = destination > 0 ? remaining[destination - 1].id : nil
        model.dispatch(.move(id: movingID, after: afterID))
    }
}

/// The visual due-date chip — calendar icon on a capsule background. Used
/// as plain decoration on task rows, and (wrapped in `DueBadge` below) as
/// the tappable quick-add detection indicator, so a due date looks the
/// same wherever it's shown.
struct DueChip: SwiftUI.View {
    let label: String
    var tint: Color = .accentColor

    var body: some SwiftUI.View {
        Label(label, systemImage: "calendar")
            .font(.caption)
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(tint.opacity(0.15), in: Capsule())
    }
}

/// A tappable `DueChip` showing a live-detected due date. Tapping it calls
/// `onDismiss` — the caller is responsible for clearing the detection
/// state that produced `label`.
struct DueBadge: SwiftUI.View {
    let label: String
    let onDismiss: () -> Void

    var body: some SwiftUI.View {
        Button(action: onDismiss) {
            DueChip(label: label)
        }
        .buttonStyle(.plain)
    }
}
