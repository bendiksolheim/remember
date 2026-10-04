import SwiftUI
import TodoKit

/// List management: create, rename, delete, and reorder lists. Kept in
/// Settings rather than the capture panel's `ListPillRow` -- that control
/// stays fast and minimal (switch only); this is the deliberate,
/// separate-visit surface for the less-frequent management actions.
struct ListsSettingsView: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    @State private var newListName = ""

    var body: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 12) {
            List {
                ForEach(model.snapshot?.lists ?? [], id: \.id) { list in
                    ListSettingsRow(
                        list: list,
                        canDelete: (model.snapshot?.lists.count ?? 0) > 1
                    )
                }
                .onMove(perform: move)
            }
            .frame(minHeight: 160, maxHeight: 280)

            HStack {
                TextField("New list", text: $newListName)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit(addList)
                Button("Add", action: addList)
                    .disabled(newListName.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 320)
    }

    private func addList() {
        model.dispatch(.addList(name: newListName, after: nil))
        newListName = ""
    }

    /// `MoveList`'s `after` is relational, same convention as `Move` --
    /// mirrors `TaskListView.move(from:to:)`'s own resolution against a
    /// snapshot.
    private func move(from source: IndexSet, to destination: Int) {
        guard let lists = model.snapshot?.lists, let sourceIndex = source.first else { return }
        let movingID = lists[sourceIndex].id
        let remaining = lists.enumerated()
            .filter { $0.offset != sourceIndex }
            .map(\.element)
        let afterID = destination > 0 ? remaining[min(destination, remaining.count) - 1].id : nil
        model.dispatch(.moveList(id: movingID, after: afterID))
    }
}

/// One editable row. `name` is seeded from `list.name` once at row creation
/// (same tradeoff `TaskDetailView` already accepts for its title field) --
/// a rename landing from another device while this row is on screen won't
/// visibly update until the row is recreated.
private struct ListSettingsRow: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    let list: ListRow
    let canDelete: Bool

    @State private var name: String

    init(list: ListRow, canDelete: Bool) {
        self.list = list
        self.canDelete = canDelete
        _name = State(initialValue: list.name)
    }

    var body: some SwiftUI.View {
        HStack {
            Circle()
                .fill(color(for: list.color))
                .frame(width: 10, height: 10)
            TextField("", text: $name)
                .textFieldStyle(.plain)
                .onChange(of: name) { _, newValue in
                    model.dispatch(.renameList(id: list.id, name: newValue))
                }
            Spacer()
            if canDelete {
                Button(role: .destructive) {
                    model.dispatch(.deleteList(id: list.id))
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.plain)
            }
        }
    }
}
