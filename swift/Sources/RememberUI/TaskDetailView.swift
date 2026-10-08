import SwiftUI
import RememberKit

public struct TaskDetailView: SwiftUI.View {
    let row: TaskRow
    @Environment(RememberModel.self) private var model

    @State private var title: String
    @State private var notes: String
    @State private var hasDue: Bool
    @State private var due: Date

    public init(row: TaskRow) {
        self.row = row
        _title = State(initialValue: row.title)
        _notes = State(initialValue: row.notes)
        _hasDue = State(initialValue: row.due != nil)
        _due = State(
            initialValue: row.due.map { Date(timeIntervalSince1970: TimeInterval($0)) } ?? Date()
        )
    }

    public var body: some SwiftUI.View {
        Form {
            TextField("Title", text: $title)
            TextField("Notes", text: $notes, axis: .vertical)
            Toggle("Due date", isOn: $hasDue)
            if hasDue {
                DatePicker("Due", selection: $due, displayedComponents: .date)
            }
            // Point-and-click equivalent of Mac's ⌘⌥2–⌘⌥9 (CaptureView) —
            // this is iOS's only way to re-categorize an existing task.
            Picker("List", selection: listBinding) {
                ForEach(model.snapshot?.lists ?? [], id: \.id) { list in
                    Text(list.name).tag(list.id)
                }
            }
        }
        .navigationTitle(row.title)
        .onChange(of: title) { _, newValue in
            model.dispatch(.setTitle(id: row.id, title: newValue))
        }
        .onChange(of: notes) { _, newValue in
            model.dispatch(.setNotes(id: row.id, notes: newValue))
        }
        .onChange(of: hasDue) { _, newValue in
            model.dispatch(.setDue(id: row.id, due: newValue ? Int64(due.timeIntervalSince1970) : nil))
        }
        .onChange(of: due) { _, newValue in
            guard hasDue else { return }
            model.dispatch(.setDue(id: row.id, due: Int64(newValue.timeIntervalSince1970)))
        }
    }

    private var listBinding: Binding<String> {
        Binding(
            get: { row.listId },
            set: { model.dispatch(.setList(id: row.id, listId: $0)) }
        )
    }
}
