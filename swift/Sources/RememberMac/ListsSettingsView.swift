import SwiftUI
import RememberKit

/// List management: create, rename, delete, and reorder lists. Kept in
/// Settings rather than the capture panel's `ListPillRow` -- that control
/// stays fast and minimal (switch only); this is the deliberate,
/// separate-visit surface for the less-frequent management actions.
struct ListsSettingsView: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    @Environment(SettingsNavigation.self) private var navigation
    @State private var newListName = ""
    @FocusState private var isNewListFocused: Bool

    private var lists: [ListRow] { model.snapshot?.lists ?? [] }

    var body: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Your lists appear as buttons under the text field when you open Remember. The first nine can be picked with ⌘1–⌘9, in the order shown here. Drag a list to move it, and click a name to rename it.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            List {
                ForEach(Array(lists.enumerated()), id: \.element.id) { index, list in
                    ListSettingsRow(
                        list: list,
                        shortcutDigit: index < 9 ? index + 1 : nil,
                        canDelete: lists.count > 1
                    )
                }
                .onMove(perform: move)
            }
            .listStyle(.bordered(alternatesRowBackgrounds: true))

            HStack {
                TextField("New list", text: $newListName)
                    .textFieldStyle(.roundedBorder)
                    .focused($isNewListFocused)
                    .onSubmit(addList)
                Button("Add List", action: addList)
                    .disabled(newListName.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .onChange(of: navigation.focusNewList, initial: true) { _, requested in
            guard requested else { return }
            navigation.focusNewList = false
            // After this update, so the field is actually in the window
            // when it's asked to take focus (on the first open it isn't yet).
            Task { @MainActor in isNewListFocused = true }
        }
    }

    private func addList() {
        guard !newListName.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        model.dispatch(.addList(name: newListName, after: nil))
        newListName = ""
    }

    /// `MoveList`'s `after` is relational, same convention as `Move` --
    /// mirrors `TaskListView.move(from:to:)`'s own resolution against a
    /// snapshot.
    private func move(from source: IndexSet, to destination: Int) {
        guard let sourceIndex = source.first else { return }
        let movingID = lists[sourceIndex].id
        let remaining = lists.enumerated()
            .filter { $0.offset != sourceIndex }
            .map(\.element)
        let afterID = destination > 0 ? remaining[min(destination, remaining.count) - 1].id : nil
        model.dispatch(.moveList(id: movingID, after: afterID))
    }
}

/// One editable row. A rename is dispatched when editing ends (Return or
/// leaving the field), not on every keystroke: each `RenameList` is its own
/// undo step and sync op, so per-keystroke dispatch made ⌘Z undo a rename
/// one letter at a time.
private struct ListSettingsRow: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    let list: ListRow
    let shortcutDigit: Int?
    let canDelete: Bool

    @State private var name: String
    @State private var isConfirmingDelete = false
    @FocusState private var isEditing: Bool

    init(list: ListRow, shortcutDigit: Int?, canDelete: Bool) {
        self.list = list
        self.shortcutDigit = shortcutDigit
        self.canDelete = canDelete
        _name = State(initialValue: list.name)
    }

    var body: some SwiftUI.View {
        HStack(spacing: 10) {
            Image(systemName: "line.3.horizontal")
                .foregroundStyle(.tertiary)
                .help("Drag to reorder")
            Circle()
                .fill(color(for: list.color))
                .frame(width: 10, height: 10)
            TextField("List name", text: $name)
                .textFieldStyle(.plain)
                .focused($isEditing)
                .onSubmit(commitRename)
            Spacer()
            if let shortcutDigit {
                Text("⌘\(shortcutDigit)")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            if canDelete {
                Button {
                    // An empty list has nothing to lose, so no question.
                    if list.taskCount == 0 {
                        model.dispatch(.deleteList(id: list.id))
                    } else {
                        isConfirmingDelete = true
                    }
                } label: {
                    Image(systemName: "trash")
                }
                .buttonStyle(.borderless)
                .help("Delete list")
            }
        }
        .padding(.vertical, 2)
        .onChange(of: isEditing) { _, editing in
            if !editing { commitRename() }
        }
        // A rename from another device shows up, unless the user is
        // in the middle of typing a name of their own.
        .onChange(of: list.name) { _, newName in
            if !isEditing { name = newName }
        }
        .alert(deleteTitle, isPresented: $isConfirmingDelete) {
            Button("Delete", role: .destructive) {
                model.dispatch(.deleteList(id: list.id))
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This deletes the list and every task in it, including completed ones.")
        }
    }

    private var deleteTitle: String {
        let tasks = list.taskCount == 1 ? "1 task" : "\(list.taskCount) tasks"
        return "Delete “\(list.name)” and its \(tasks)?"
    }

    /// An empty name goes back to the current one instead of being sent
    /// (the core would ignore it anyway, leaving the field blank).
    private func commitRename() {
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty {
            name = list.name
        } else if trimmed != list.name {
            model.dispatch(.renameList(id: list.id, name: trimmed))
        }
    }
}
