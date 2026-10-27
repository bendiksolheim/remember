import SwiftUI
import RememberKit

/// The whole iOS app: the iPhone version of the Mac's capture panel
/// (`CaptureView`). A quick-add field that's focused whenever the app comes
/// to the front, the list pills under it, then the current list's open
/// tasks. A ticked-off row stays, done, in place for a second after the
/// last tick -- remember-core holds it until `releaseHeld`.
///
/// Touch stands in for the Mac's keys: tap a row for its details, swipe
/// right to add steps to it ("a"), swipe left to delete, long-press for
/// everything else (Tab / ⇧Tab, ⌘⌥1–9), drag to reorder (⌥⇧J/K), and
/// shake to undo (⌘Z).
public struct TaskListView: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @State private var input = ""
    /// Live result of `RememberModel.detectDue` for `input`, re-checked on
    /// every keystroke. `nil` means no recognized trailing phrase.
    @State private var dueDetection: DueDetection?
    /// Set when the user taps the badge to reject a detection for this
    /// input — cleared again on the next keystroke, so a fresh phrase is
    /// never silently suppressed by an old dismissal.
    @State private var dueDismissed = false
    /// The task captures go into as steps (swipe right on a row; shown as a
    /// chip). Survives adding; cleared by tapping the chip, switching list,
    /// or the task leaving the list. `CaptureView.addTargetID`.
    @State private var addTargetID: String?
    @FocusState private var inputFocused: Bool
    /// Releases held completed rows a second after the last toggle, so a
    /// run of check-offs leaves together -- same as `CaptureView`.
    @State private var releaseTask: Task<Void, Never>?
    @State private var detail: DetailTarget?
    @State private var toast: Toast?
    @State private var isAskingToUndo = false
    @State private var settings: SettingsRequest?

    public init() {}

    private var rows: [TaskRow] { model.snapshot?.rows ?? [] }
    private var lists: [ListRow] { model.snapshot?.lists ?? [] }
    private var canUndo: Bool { model.snapshot?.canUndo ?? false }
    private var canRedo: Bool { model.snapshot?.canRedo ?? false }

    /// The open row `addTargetID` names, or `nil` once it's gone.
    private var addTarget: TaskRow? {
        rows.first { $0.id == addTargetID && !$0.done }
    }

    public var body: some SwiftUI.View {
        List {
            HStack(spacing: 8) {
                if let target = addTarget {
                    ParentChip(title: target.title) { addTargetID = nil }
                }
                TextField(addTarget == nil ? "Add a task…" : "Add a step…", text: $input)
                    .font(.body)
                    .focused($inputFocused)
                    .submitLabel(.done)
                    .onSubmit(addTask)
                    .onChange(of: input) { _, newValue in
                        dueDetection = newValue.isEmpty ? nil : model.detectDue(newValue)
                        dueDismissed = false
                    }
                if let detection = dueDetection, !dueDismissed {
                    DueBadge(label: detection.label) { dueDismissed = true }
                }
            }
            .listRowSeparator(.hidden)

            ListPillRow(
                onSelect: { listId in
                    addTargetID = nil
                    model.setCurrentList(listId)
                },
                onAdd: { settings = SettingsRequest(focusNewList: true) }
            )
            .listRowInsets(EdgeInsets())
            .listRowSeparator(.hidden)

            if rows.isEmpty {
                Text("Nothing here. Add a task above.")
                    .foregroundStyle(.secondary)
                    .listRowSeparator(.hidden)
            }
            ForEach(rows, id: \.id) { row in
                TaskRowView(row: row, onToggle: { toggle(row) }, onOpen: { open(row) })
                    .swipeActions(edge: .leading) {
                        if !row.done {
                            Button { target(row) } label: {
                                Label("Add Step", systemImage: "text.badge.plus")
                            }
                            .tint(.accentColor)
                        }
                    }
                    .swipeActions(edge: .trailing) {
                        Button(role: .destructive) { delete(row) } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
                    .contextMenu { menu(for: row) }
            }
            .onMove(perform: move)
        }
        .listStyle(.plain)
        // The field grabs focus on launch, so scrolling is the way out.
        .scrollDismissesKeyboard(.immediately)
        .animation(.easeOut(duration: 0.25), value: rows.map(\.id))
        .navigationTitle("Remember")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { settings = SettingsRequest(focusNewList: false) } label: {
                    Label("Settings", systemImage: "gearshape")
                }
            }
        }
        .safeAreaInset(edge: .bottom) {
            if let toast {
                ToastView(message: toast.message) {
                    self.toast = nil
                    model.dispatch(.undo)
                }
                .padding()
                .transition(.move(edge: .bottom).combined(with: .opacity))
                .task(id: toast.id) {
                    try? await Task.sleep(for: .seconds(4))
                    guard !Task.isCancelled else { return }
                    withAnimation { self.toast = nil }
                }
            }
        }
        .animation(.easeOut(duration: 0.2), value: toast?.id)
        .sheet(item: $detail) { detail in
            TaskDetailView(row: detail.row) { delete(detail.row) }
                .environment(model)
        }
        .sheet(item: $settings) { request in
            SettingsView(focusNewList: request.focusNewList)
                .environment(model)
        }
        .alert(canUndo ? "Undo the last change?" : "Redo what you undid?", isPresented: $isAskingToUndo) {
            if canUndo {
                Button("Undo") { perform(.undo) }
            }
            if canRedo {
                Button("Redo") { perform(.redo) }
            }
            Button("Cancel", role: .cancel) {}
        }
        .onReceive(NotificationCenter.default.publisher(for: .rememberDeviceDidShake)) { _ in
            // Shaking while typing is iOS's own "Undo Typing".
            guard !(inputFocused && !input.isEmpty), canUndo || canRedo else { return }
            isAskingToUndo = true
        }
        .onChange(of: model.snapshot?.revision) { _, _ in
            // The target is gone (completed, deleted, moved, synced away).
            if addTargetID != nil, addTarget == nil {
                addTargetID = nil
            }
        }
        // Ready for a new task whenever the app comes to the front.
        .onChange(of: scenePhase, initial: true) { _, newPhase in
            if newPhase == .active {
                inputFocused = true
            }
        }
    }

    /// Long-press: everything a row can do, including what the Mac does
    /// with Tab / ⇧Tab and ⌘⌥1–9.
    @ViewBuilder
    private func menu(for row: TaskRow) -> some SwiftUI.View {
        if !row.done {
            Button { target(row) } label: {
                Label("Add Step", systemImage: "text.badge.plus")
            }
            // remember-core decides where it goes, and ignores what can't
            // be done (e.g. a task that has steps of its own).
            if row.parentId == nil, row.stepCount == 0, row.id != rows.first?.id {
                Button { perform(.indent(id: row.id)) } label: {
                    Label("Make Step of Task Above", systemImage: "increase.indent")
                }
            }
            if row.parentId != nil {
                Button { perform(.outdent(id: row.id)) } label: {
                    Label("Make Task", systemImage: "decrease.indent")
                }
            }
            if lists.count > 1 {
                Menu {
                    ForEach(lists.filter { $0.id != row.listId }, id: \.id) { list in
                        Button(list.name) { moveToList(row, list) }
                    }
                } label: {
                    Label("Move to List", systemImage: "folder")
                }
            }
        }
        Divider()
        Button(role: .destructive) { delete(row) } label: {
            Label("Delete", systemImage: "trash")
        }
    }

    /// Every change made from this screen goes through here: a toast's
    /// Undo is only right while its change is the last one.
    private func perform(_ command: Command) {
        toast = nil
        model.dispatch(command)
    }

    private func addTask() {
        let title = input.trimmingCharacters(in: .whitespaces)
        if !title.isEmpty {
            let parentId = addTarget?.id
            if let detection = dueDetection, !dueDismissed {
                perform(.add(title: detection.strippedTitle, after: nil, due: detection.due, listId: nil, parentId: parentId))
            } else {
                perform(.add(title: input, after: nil, due: nil, listId: nil, parentId: parentId))
            }
        }
        input = ""
        dueDetection = nil
        dueDismissed = false
        // Return ends editing on iOS; keep the keyboard up for the next one.
        inputFocused = true
    }

    private func toggle(_ row: TaskRow) {
        perform(.setDone(id: row.id, done: !row.done))
        releaseTask?.cancel()
        releaseTask = Task {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            releaseTask = nil
            model.releaseHeld()
        }
    }

    /// Details are for open rows; a done one is on its way out.
    private func open(_ row: TaskRow) {
        guard !row.done, !row.contextOnly else { return }
        detail = DetailTarget(row: row)
    }

    /// Aims the input at the row's task, like the Mac's "a".
    private func target(_ row: TaskRow) {
        addTargetID = row.parentId ?? row.id
        inputFocused = true
    }

    private func delete(_ row: TaskRow) {
        perform(.delete(id: row.id))
        toast = Toast(message: row.stepCount > 0
            ? "Deleted “\(row.title)” and its steps"
            : "Deleted “\(row.title)”")
    }

    private func moveToList(_ row: TaskRow, _ list: ListRow) {
        perform(.setList(id: row.id, listId: list.id))
        toast = Toast(message: "Moved to \(list.name)")
    }

    /// `destination` follows the same convention as
    /// `Array.move(fromOffsets:toOffset:)`: a post-removal insertion index.
    /// No local reordered array is kept — this just resolves `destination`
    /// to the id it should land after and dispatches, same as every other
    /// mutation here; the next snapshot redraws the list. remember-core
    /// refuses a step dragged out of its task, so it springs back.
    private func move(from source: IndexSet, to destination: Int) {
        guard let sourceIndex = source.first else { return }
        let movingID = rows[sourceIndex].id
        let remaining = rows.enumerated()
            .filter { $0.offset != sourceIndex }
            .map(\.element)
        let afterID = destination > 0 ? remaining[min(destination, remaining.count) - 1].id : nil
        perform(.move(id: movingID, after: afterID))
    }
}

/// The row a detail sheet is showing, as it was when opened -- the sheet
/// keeps working if the row then leaves this list.
private struct DetailTarget: Identifiable {
    let row: TaskRow
    var id: String { row.id }
}

/// The settings sheet, and whether it opens ready to name a new list.
private struct SettingsRequest: Identifiable {
    let id = UUID()
    let focusNewList: Bool
}

/// A short-lived "what just happened" note with an Undo, for changes that
/// take a row out of sight (delete, move to another list).
private struct Toast {
    let id = UUID()
    let message: String
}

private struct ToastView: SwiftUI.View {
    let message: String
    let onUndo: () -> Void

    var body: some SwiftUI.View {
        HStack(spacing: 12) {
            Text(message)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
            Button("Undo", action: onUndo)
                .fontWeight(.semibold)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: Capsule())
        .shadow(radius: 8, y: 2)
    }
}

/// "Tax return ›" in front of the quick-add field while adding steps (see
/// `TaskListView.addTargetID`). Tapping it drops the target.
private struct ParentChip: SwiftUI.View {
    let title: String
    let onClear: () -> Void

    var body: some SwiftUI.View {
        Button(action: onClear) {
            HStack(spacing: 4) {
                Text(title)
                    .lineLimit(1)
                    .frame(maxWidth: 140, alignment: .leading)
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
            }
            .font(.subheadline.weight(.medium))
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(Color.accentColor.opacity(0.15), in: Capsule())
            .foregroundStyle(Color.accentColor)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Adding steps to \(title)")
        .accessibilityHint("Tap to add tasks instead")
    }
}

/// The lists as a sideways-scrolling row of pills -- the Mac's
/// `ListPillRow`, minus the ⌘-digit hints. Tapping one switches the list
/// new tasks go into and the rows below show (`onSelect`).
struct ListPillRow: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    let onSelect: (String) -> Void
    /// The trailing "+": opens Settings, ready to name a new list.
    let onAdd: () -> Void

    var body: some SwiftUI.View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(model.snapshot?.lists ?? [], id: \.id) { list in
                    ListPill(
                        name: list.name,
                        isSelected: model.snapshot?.currentList == list.id,
                        color: color(for: list.color),
                        textColor: pillTextColor(for: list.color)
                    ) {
                        onSelect(list.id)
                    }
                }
                Button(action: onAdd) {
                    Image(systemName: "plus")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(.secondary)
                        .padding(8)
                        .background(Color.secondary.opacity(0.12), in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("New List")
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 6)
        }
    }
}

/// Same two states as the Mac pill: an outline at rest, a solid fill when
/// selected (`textColor` keeps the name readable on it).
private struct ListPill: SwiftUI.View {
    let name: String
    let isSelected: Bool
    let color: Color
    let textColor: Color
    let action: () -> Void

    var body: some SwiftUI.View {
        Button(action: action) {
            Text(name)
                .font(.subheadline.weight(.medium))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(isSelected ? color : Color.clear, in: Capsule())
                .overlay {
                    if !isSelected {
                        Capsule().strokeBorder(color, lineWidth: 1)
                    }
                }
                .foregroundStyle(isSelected ? textColor : color)
                .contentShape(Capsule())
        }
        // `.plain`, or the List row would swallow the tap as its own.
        .buttonStyle(.plain)
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
