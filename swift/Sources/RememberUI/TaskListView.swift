import SwiftUI
import RememberKit

/// The whole iOS app: one page per list, tinted in the list's color. A
/// large title (tap it for the lists; swipe it sideways for the next one)
/// over the list's open tasks in a card, and the quick-add field pinned
/// above the keyboard, focused whenever the app comes to the front. A
/// ticked-off row stays, done, in place for a second after the last tick
/// -- remember-core holds it until `releaseHeld`.
///
/// Touch stands in for the Mac's keys: tap a row for its details, swipe
/// right to add steps to it ("a"), swipe left to delete, long-press for
/// everything else (Tab / ⇧Tab, ⌘⌥1–9), drag to reorder (⌥⇧J/K), and
/// shake to undo (⌘Z). Only the title switches lists by swiping: rows
/// keep their own swipes.
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
    /// Room between the field and the keyboard, measured by eye on a
    /// device: 8 pt of gap plus what the keyboard overlaps.
    private static let keyboardGap: CGFloat = 22
    /// Where the next list's page slides in from: the side it sits on.
    @State private var pageEdge: Edge = .trailing

    public init() {}

    private var rows: [TaskRow] { model.snapshot?.rows ?? [] }
    private var lists: [ListRow] { model.snapshot?.lists ?? [] }
    private var currentListID: String? { model.snapshot?.currentList }
    private var currentList: ListRow? { lists.first { $0.id == currentListID } }
    private var listColor: Color { currentList.map { color(for: $0.color) } ?? .accentColor }
    private var canUndo: Bool { model.snapshot?.canUndo ?? false }
    private var canRedo: Bool { model.snapshot?.canRedo ?? false }

    /// The open row `addTargetID` names, or `nil` once it's gone.
    private var addTarget: TaskRow? {
        rows.first { $0.id == addTargetID && !$0.done }
    }

    public var body: some SwiftUI.View {
        ZStack {
            // A new identity per list, so switching slides one page out and
            // the next in (`pageEdge`).
            page
                .id(currentListID)
                .transition(.push(from: pageEdge))
        }
        .animation(.snappy, value: currentListID)
        .background(PageBackground(color: listColor).ignoresSafeArea())
        .tint(listColor)
        // Not for the first snapshot at launch.
        .sensoryFeedback(.selection, trigger: currentListID) { old, _ in old != nil }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                PageDots(lists: lists, currentListID: currentListID, onSelect: switchList)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { settings = SettingsRequest(focusNewList: false) } label: {
                    Label("Settings", systemImage: "gearshape")
                }
                .tint(.primary)
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            VStack(spacing: 8) {
                if let toast {
                    ToastView(message: toast.message) {
                        self.toast = nil
                        model.dispatch(.undo)
                    }
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                    .task(id: toast.id) {
                        try? await Task.sleep(for: .seconds(4))
                        guard !Task.isCancelled else { return }
                        withAnimation { self.toast = nil }
                    }
                }
                composer
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
            // The keyboard reaches a little above the edge it reports, so
            // without more room the field sits partly under it. The field
            // is the only one on this screen, so its focus says whether the
            // keyboard is up; down, the home indicator's margin is enough.
            .padding(.bottom, inputFocused ? Self.keyboardGap : 8)
            .animation(.snappy, value: inputFocused)
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

    /// One list's page: its title, then its rows in a card.
    private var page: some SwiftUI.View {
        List {
            Section {
                if let list = currentList {
                    ListHeader(
                        list: list,
                        lists: lists,
                        onSelect: switchList,
                        onEditLists: { settings = SettingsRequest(focusNewList: false) },
                        onNewList: { settings = SettingsRequest(focusNewList: true) }
                    )
                    // Sideways on the title turns the page. Only there:
                    // on a row the same swipe adds a step or deletes.
                    .simultaneousGesture(
                        DragGesture(minimumDistance: 24).onEnded { value in
                            let dx = value.translation.width
                            guard abs(dx) > 60, abs(dx) > abs(value.translation.height) * 1.5 else { return }
                            turnPage(by: dx < 0 ? 1 : -1)
                        }
                    )
                }
            }
            .listRowBackground(Color.clear)
            .listRowInsets(EdgeInsets(top: 4, leading: 20, bottom: 0, trailing: 20))

            Section {
                if rows.isEmpty {
                    Text("Nothing here. Add a task below.")
                        .foregroundStyle(.secondary)
                }
                ForEach(rows, id: \.id) { row in
                    TaskRowView(row: row, onToggle: { toggle(row) }, onOpen: { open(row) })
                        .swipeActions(edge: .leading) {
                            if !row.done {
                                Button { target(row) } label: {
                                    Label("Add Step", systemImage: "text.badge.plus")
                                }
                                .tint(listColor)
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
        }
        .listStyle(.insetGrouped)
        .listSectionSpacing(.compact)
        .scrollContentBackground(.hidden)
        // The field grabs focus on launch, so scrolling is the way out.
        .scrollDismissesKeyboard(.immediately)
        .animation(.easeOut(duration: 0.25), value: rows.map(\.id))
    }

    /// The quick-add field, floating above the keyboard like Messages': one
    /// glass capsule with nothing behind it, so the rows scroll under it.
    /// Inside: the step target's chip, the field, a detected due date, and
    /// an add button once there's something to add.
    private var composer: some SwiftUI.View {
        HStack(spacing: 8) {
            if let target = addTarget {
                ParentChip(title: target.title) { addTargetID = nil }
            }
            TextField(addPrompt, text: $input)
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
            if !isInputBlank {
                Button(action: addTask) {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(
                            currentList.map { pillTextColor(for: $0.color) } ?? .white,
                            listColor
                        )
                }
                .buttonStyle(.plain)
                .accessibilityLabel(addTarget == nil ? "Add Task" : "Add Step")
                .transition(.scale.combined(with: .opacity))
            }
        }
        .padding(.leading, 16)
        .padding(.trailing, 6)
        .frame(minHeight: 44)
        .animation(.snappy(duration: 0.2), value: isInputBlank)
        .modifier(GlassCapsule())
    }

    private var isInputBlank: Bool {
        input.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private var addPrompt: String {
        if addTarget != nil { return "Add a step…" }
        return currentList.map { "Add to \($0.name)…" } ?? "Add a task…"
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

    /// Shows `listId`'s page, sliding it in from the side it sits on.
    private func switchList(_ listId: String) {
        guard listId != currentListID,
              let to = lists.firstIndex(where: { $0.id == listId })
        else { return }
        let from = lists.firstIndex { $0.id == currentListID } ?? 0
        pageEdge = to > from ? .trailing : .leading
        addTargetID = nil
        model.setCurrentList(listId)
    }

    /// The next (`1`) or previous (`-1`) list's page; none past the ends.
    private func turnPage(by step: Int) {
        guard let index = lists.firstIndex(where: { $0.id == currentListID }),
              lists.indices.contains(index + step)
        else { return }
        switchList(lists[index + step].id)
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
        .modifier(GlassCapsule())
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
            .background(.tint.opacity(0.15), in: Capsule())
            .foregroundStyle(.tint)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Adding steps to \(title)")
        .accessibilityHint("Tap to add tasks instead")
    }
}

/// The page's title: the list's name in its color -- a menu of every list,
/// plus list settings -- over what the list has left to do.
private struct ListHeader: SwiftUI.View {
    @Environment(\.colorScheme) private var colorScheme
    let list: ListRow
    let lists: [ListRow]
    let onSelect: (String) -> Void
    let onEditLists: () -> Void
    let onNewList: () -> Void

    var body: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 2) {
            Menu {
                ForEach(lists, id: \.id) { other in
                    Button { onSelect(other.id) } label: {
                        if other.id == list.id {
                            Label(other.name, systemImage: "checkmark")
                        } else {
                            Text(other.name)
                        }
                    }
                }
                Divider()
                Button(action: onNewList) {
                    Label("New List", systemImage: "plus")
                }
                Button(action: onEditLists) {
                    Label("Edit Lists", systemImage: "pencil")
                }
            } label: {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(list.name)
                        .font(.largeTitle.bold())
                        .lineLimit(1)
                    Image(systemName: "chevron.down")
                        .font(.title3.weight(.semibold))
                }
                .foregroundStyle(color(for: list.color))
                // System colors are tuned for fills: darken the title in
                // light mode so yellow or mint still reads on the tint.
                .brightness(colorScheme == .light ? -0.2 : 0)
            }
            // Plain, or the List row would take every tap on it as its own.
            .menuStyle(.button)
            .buttonStyle(.plain)
            .accessibilityHint("Shows your lists")

            Text(summary)
                .font(.subheadline)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // The whole header takes the page-turning swipe, not just the text.
        .contentShape(Rectangle())
    }

    /// "8 open · 1 due today · 1 overdue", leaving out what's zero.
    private var summary: String {
        guard list.activeCount > 0 else { return "All done" }
        var parts = ["\(list.activeCount) open"]
        if list.dueTodayCount > 0 { parts.append("\(list.dueTodayCount) due today") }
        if list.overdueCount > 0 { parts.append("\(list.overdueCount) overdue") }
        return parts.joined(separator: " · ")
    }
}

/// One dot per list, in each list's color, the current one drawn long.
/// Tapping a dot goes to that list's page. Nothing for a single list.
private struct PageDots: SwiftUI.View {
    let lists: [ListRow]
    let currentListID: String?
    let onSelect: (String) -> Void

    var body: some SwiftUI.View {
        if lists.count > 1 {
            HStack(spacing: 6) {
                ForEach(lists, id: \.id) { list in
                    let isCurrent = list.id == currentListID
                    Button { onSelect(list.id) } label: {
                        Capsule()
                            .fill(color(for: list.color))
                            .frame(width: isCurrent ? 18 : 7, height: 7)
                            .opacity(isCurrent ? 1 : 0.45)
                            .padding(.vertical, 12)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(list.name)
                    .accessibilityAddTraits(isCurrent ? .isSelected : [])
                }
            }
            .animation(.snappy, value: currentListID)
        }
    }
}

/// Behind a list's page: the grouped background, washed with its color.
private struct PageBackground: SwiftUI.View {
    let color: Color

    var body: some SwiftUI.View {
        ZStack {
            Color(uiColor: .systemGroupedBackground)
            color.opacity(0.12)
        }
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

/// Liquid Glass where there is some (iOS 26), a material capsule before.
private struct GlassCapsule: ViewModifier {
    func body(content: Content) -> some SwiftUI.View {
        #if compiler(>=6.2)
        if #available(iOS 26, *) {
            content.glassEffect(.regular.interactive(), in: Capsule())
        } else {
            fallback(content)
        }
        #else
        fallback(content)
        #endif
    }

    private func fallback(_ content: Content) -> some SwiftUI.View {
        content
            .background(.regularMaterial, in: Capsule())
            .shadow(color: .black.opacity(0.08), radius: 8, y: 2)
    }
}
