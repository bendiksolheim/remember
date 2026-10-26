import SwiftUI
import RememberKit

/// The sheet behind a tap on a task: title, notes, due date and list --
/// what the Mac can only set while capturing. Title and notes are saved
/// when editing ends (Return, or closing the sheet), not per keystroke, so
/// an undo takes back a whole edit rather than one character.
public struct TaskDetailView: SwiftUI.View {
    let row: TaskRow
    let onDelete: () -> Void
    @Environment(RememberModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var title: String
    /// What `commitTitle` last saved, so Return and then closing the sheet
    /// don't save it twice.
    @State private var savedTitle: String
    @State private var notes: String
    @State private var hasDue: Bool
    @State private var due: Date
    @State private var listId: String
    /// Nothing left to save once the task is deleted.
    @State private var isDeleted = false

    public init(row: TaskRow, onDelete: @escaping () -> Void) {
        self.row = row
        self.onDelete = onDelete
        _title = State(initialValue: row.title)
        _savedTitle = State(initialValue: row.title)
        _notes = State(initialValue: row.notes)
        _hasDue = State(initialValue: row.due != nil)
        _due = State(
            initialValue: row.due.map { Date(timeIntervalSince1970: TimeInterval($0)) } ?? Date()
        )
        _listId = State(initialValue: row.listId)
    }

    public var body: some SwiftUI.View {
        NavigationStack {
            Form {
                Section {
                    TextField("Title", text: $title)
                        .onSubmit(commitTitle)
                    TextField("Notes", text: $notes, axis: .vertical)
                        .lineLimit(3...)
                }
                Section {
                    Toggle("Due Date", isOn: $hasDue)
                    if hasDue {
                        DatePicker("Due", selection: $due, displayedComponents: .date)
                    }
                    Picker("List", selection: $listId) {
                        ForEach(model.snapshot?.lists ?? [], id: \.id) { list in
                            Text(list.name).tag(list.id)
                        }
                    }
                }
                Section {
                    Button(row.stepCount > 0 ? "Delete Task and Steps" : "Delete", role: .destructive) {
                        isDeleted = true
                        dismiss()
                        onDelete()
                    }
                }
            }
            .navigationTitle(row.parentId == nil ? "Task" : "Step")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .onDisappear {
            guard !isDeleted else { return }
            commitTitle()
            if notes != row.notes {
                model.dispatch(.setNotes(id: row.id, notes: notes))
            }
        }
        .onChange(of: hasDue) { _, newValue in
            model.dispatch(.setDue(id: row.id, due: newValue ? Int64(due.timeIntervalSince1970) : nil))
        }
        .onChange(of: due) { _, newValue in
            guard hasDue else { return }
            model.dispatch(.setDue(id: row.id, due: Int64(newValue.timeIntervalSince1970)))
        }
        .onChange(of: listId) { _, newValue in
            model.dispatch(.setList(id: row.id, listId: newValue))
        }
    }

    /// `SetTitle` trims, and ignores an empty title, itself.
    private func commitTitle() {
        guard title != savedTitle else { return }
        model.dispatch(.setTitle(id: row.id, title: title))
        savedTitle = title
    }
}
