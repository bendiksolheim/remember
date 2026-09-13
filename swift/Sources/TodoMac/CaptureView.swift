import AppKit
import SwiftUI
import TodoKit

// Same qualification convention as TodoUI (see TaskRowView.swift): TodoKit's
// generated `App`/`View` collide by name with `SwiftUI.App`/`SwiftUI.View`
// once both modules are imported, so conformances/opaque returns are
// qualified explicitly rather than trusting contextual resolution --
// unverified since there's no compiler in this container.

/// The entire macOS UI for v1: a quick-add field, the live incomplete task
/// list, and inline title editing ("e" on the focused row), hosted inside
/// `SpotlightPanel`. Deleting and reordering are still out of scope here --
/// those remain reachable only via `TodoUI`'s `TaskListView`, which iOS
/// still uses but this target no longer wires up.
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
    /// Unifies the quick-add field and every row into one keyboard-focus
    /// chain: alt+j/alt+k walk `focusChain` (input first, then rows in
    /// display order) and wrap at both ends. `fileprivate`, not `private`,
    /// so `CaptureTaskRow` below (a sibling type, not an extension of this
    /// one) can name it in its own `FocusState<...>.Binding` parameter.
    fileprivate enum FocusTarget: Hashable {
        case input
        case row(String)
        case editingRow(String)
    }
    @FocusState private var focus: FocusTarget?
    /// Id of the row currently in inline-edit mode, and its draft text.
    /// `nil` means no row is being edited. Centralized here (not per-row
    /// local state) so the alt+j/alt+k monitor and the "e" shortcut's
    /// `.disabled` gate can both read/drive it directly.
    @State private var editingRowID: String?
    @State private var editText = ""
    /// Rows that were just checked off, held here (not in `model.snapshot`,
    /// which already excludes them once `View.active` is set) so they can
    /// visibly linger for ~3s instead of vanishing the instant they're
    /// marked done.
    @State private var ghosts: [TaskRow] = []
    /// Pending removal task per ghost row id, so un-completing a row (Space
    /// or click, while it's still lingering) can cancel the removal instead
    /// of racing it.
    @State private var ghostRemovalTasks: [String: Task<Void, Never>] = [:]
    /// Where each ghost was, so it renders back in its original spot instead
    /// of jumping to the top of the list while it lingers: the id of the
    /// active row it directly followed at the moment it was completed, or
    /// `""` if it was first. Reordering rows out from under a focused one
    /// is what was corrupting keyboard focus (and the panel's height calc)
    /// on complete/un-complete -- this keeps every row's position stable
    /// across the whole ghost lifecycle.
    @State private var ghostAnchors: [String: String] = [:]
    /// Ghost ids that have been un-completed but are kept in `ghosts` until
    /// `model.snapshot` actually confirms the row is active again --
    /// `TodoModel`'s snapshot updates hop through a `Task { @MainActor in
    /// ... } ` (Model.swift), landing a run-loop turn after `dispatch`
    /// returns, so dropping a ghost the instant we dispatch would make the
    /// row vanish from `displayRows` for a frame (neither still a ghost nor
    /// yet back in the live snapshot) -- a visible row-count blip that
    /// shifts the panel's height. Keeping it in `ghosts` (rendered as
    /// not-completed) until the snapshot catches up closes that gap.
    @State private var revivedIDs: Set<String> = []
    /// Local monitor for alt+j/alt+k: SwiftUI's `.keyboardShortcut` (used
    /// below for Undo/Redo/Toggle) isn't honored while the quick-add
    /// TextField is first responder for a bare-Option combo -- unlike
    /// Command-based shortcuts, Option+letter is normally live text input
    /// (it types accented/special characters), so AppKit lets the text
    /// field consume it before falling back to key-equivalent scanning. A
    /// local monitor intercepts before that happens, regardless of focus.
    @State private var optionNavMonitor: Any?

    var body: some SwiftUI.View {
        VStack(spacing: 0) {
            TextField("Add a task…", text: $input)
                .textFieldStyle(.plain)
                .font(.system(size: 22))
                .padding(.horizontal, 20)
                .padding(.vertical, 16)
                .focused($focus, equals: .input)
                .onSubmit(addTask)

            if !displayRows.isEmpty {
                Divider()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(displayRows, id: \.row.id) { entry in
                            CaptureTaskRow(
                                row: entry.row,
                                completed: entry.completed,
                                isFocused: focus == .row(entry.row.id),
                                isEditing: editingRowID == entry.row.id,
                                editText: $editText,
                                focus: $focus,
                                onToggle: { toggle(id: entry.row.id) },
                                onFocusRequest: { focus = .row(entry.row.id) },
                                onCommitEdit: commitEdit,
                                onCancelEdit: cancelEdit
                            )
                            .focusable()
                            .focusEffectDisabled()
                            .focused($focus, equals: .row(entry.row.id))
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
        .onAppear {
            focus = .input
            installOptionNavMonitor()
        }
        .onDisappear(perform: removeOptionNavMonitor)
        .onChange(of: contentHeight, initial: true) { _, newValue in
            onContentHeightChange(newValue)
        }
        .onChange(of: model.snapshot?.revision) { _, _ in reconcileRevivedGhosts() }
        .onChange(of: focus) { oldValue, _ in
            // Focus left the edit field for a reason other than our own
            // commitEdit()/cancelEdit() (both clear `editingRowID` before
            // moving focus themselves, so this guard is already false by
            // then) -- e.g. the panel lost key window status, or the user
            // clicked elsewhere. Discard the in-progress edit rather than
            // silently keep it alive.
            guard case .editingRow(let id)? = oldValue, editingRowID == id else { return }
            editingRowID = nil
            editText = ""
        }
        .overlay {
            // Hidden buttons rather than a raw NSEvent monitor: SwiftUI's
            // `.keyboardShortcut` is honored by NSHostingView's own
            // performKeyEquivalent handling even with no app-level
            // CommandGroup, so this Just Works inside the accessory-app
            // panel without extra AppKit plumbing. (alt+j/alt+k can't use
            // this trick -- see `optionNavMonitor` -- so those go through a
            // real NSEvent monitor instead.)
            Group {
                Button("Undo") { model.dispatch(.undo) }
                    .keyboardShortcut("z", modifiers: .command)
                Button("Redo") { model.dispatch(.redo) }
                    .keyboardShortcut("z", modifiers: [.command, .shift])
                // Bare Space with no modifier would otherwise swallow the
                // spacebar while the quick-add field is focused, so this is
                // disabled whenever focus isn't on a row. Same reasoning
                // keeps it disabled while a row is being edited (`isRowFocused`
                // is false for `.editingRow`), so Space types into the field
                // instead of toggling completion.
                Button("Toggle Focused") { toggleFocusedRow() }
                    .keyboardShortcut(.space, modifiers: [])
                    .disabled(!isRowFocused)
                // Bare "e" enters inline editing on the focused row. Disabled
                // for completed/ghost rows -- renaming a task that's done (or
                // mid-fade-out) isn't meaningful -- and naturally inert while
                // the quick-add field or another edit field has focus.
                Button("Edit Focused") { beginEditFocusedRow() }
                    .keyboardShortcut("e", modifiers: [])
                    .disabled(!canEditFocusedRow)
            }
            .hidden()
        }
    }

    private func installOptionNavMonitor() {
        guard optionNavMonitor == nil else { return }
        optionNavMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard event.modifierFlags.intersection(.deviceIndependentFlagsMask) == [.option] else {
                return event
            }
            switch event.charactersIgnoringModifiers {
            case "j":
                // Still swallowed (returns nil) while editing, not just
                // skipped -- letting the event through would have AppKit
                // insert its dead-key/accented character into the field
                // being edited instead of doing nothing.
                if editingRowID == nil { moveFocus(by: 1) }
                return nil
            case "k":
                if editingRowID == nil { moveFocus(by: -1) }
                return nil
            default:
                return event
            }
        }
    }

    private func removeOptionNavMonitor() {
        if let optionNavMonitor {
            NSEvent.removeMonitor(optionNavMonitor)
        }
        optionNavMonitor = nil
    }

    private var isRowFocused: Bool {
        if case .row? = focus { return true }
        return false
    }

    /// Keyboard-focus order for alt+j/alt+k: the quick-add field first, then
    /// every displayed row (ghosts included) in display order. Both ends
    /// wrap around.
    private var focusChain: [FocusTarget] {
        [.input] + displayRows.map { .row($0.row.id) }
    }

    private func moveFocus(by delta: Int) {
        let chain = focusChain
        guard !chain.isEmpty else { return }
        let currentIndex = focus.flatMap { chain.firstIndex(of: $0) } ?? 0
        let nextIndex = (currentIndex + delta + chain.count) % chain.count
        focus = chain[nextIndex]
    }

    private func toggleFocusedRow() {
        guard case .row(let id)? = focus else { return }
        toggle(id: id)
    }

    private var canEditFocusedRow: Bool {
        guard case .row(let id)? = focus,
            let entry = displayRows.first(where: { $0.row.id == id })
        else { return false }
        return !entry.row.done
    }

    private func beginEditFocusedRow() {
        guard case .row(let id)? = focus,
            let entry = displayRows.first(where: { $0.row.id == id })
        else { return }
        editingRowID = id
        editText = entry.row.title
        focus = .editingRow(id)
    }

    /// Enter finalizes: dispatches the draft unconditionally. `SetTitle`
    /// trims and no-ops on an empty result itself (see todo-core), so an
    /// emptied title just silently reverts -- no client-side empty check
    /// needed here.
    private func commitEdit() {
        guard let id = editingRowID else { return }
        model.dispatch(.setTitle(id: id, title: editText))
        endEdit(returnFocusTo: id)
    }

    /// Escape cancels: drop the draft, dispatch nothing.
    private func cancelEdit() {
        guard let id = editingRowID else { return }
        endEdit(returnFocusTo: id)
    }

    /// Leaves edit mode and hands focus back to the row. The `focus`
    /// reassignment is deferred a run-loop turn: clearing `editingRowID`
    /// swaps the edit `TextField` back out for a plain `Text` in the same
    /// update, and that field is what currently holds `focus` -- setting
    /// `focus` to `.row(id)` in that same synchronous pass loses the race
    /// against SwiftUI's own "the focused view just disappeared" reset,
    /// which clears focus to nil *after* our assignment lands. Letting the
    /// removal commit first, then reassigning focus, avoids that.
    private func endEdit(returnFocusTo id: String) {
        editingRowID = nil
        editText = ""
        DispatchQueue.main.async {
            focus = .row(id)
        }
    }

    /// Active rows in their natural order, with each ghost reinserted right
    /// after the anchor it was captured with -- never at the front -- so a
    /// row's index never changes across a complete/un-complete cycle. Any
    /// id still in `ghosts` is excluded from `active` unconditionally (not
    /// just while genuinely completed): while reviving, the row is still in
    /// `ghosts` *and* may briefly also still be in the stale snapshot, and
    /// without this filter it would render twice for a frame.
    private var displayRows: [(row: TaskRow, completed: Bool)] {
        let ghostIDs = Set(ghosts.map(\.id))
        let active = (model.snapshot?.rows ?? []).filter { !ghostIDs.contains($0.id) }
        func ghostsAnchored(to anchor: String) -> [(TaskRow, Bool)] {
            ghosts.filter { ghostAnchors[$0.id] == anchor }
                .map { ($0, !revivedIDs.contains($0.id)) }
        }

        var result = ghostsAnchored(to: "")
        for row in active {
            result.append((row, false))
            result += ghostsAnchored(to: row.id)
        }
        // A ghost whose anchor row is itself gone (e.g. also completed, or
        // deleted) has nowhere to be reinserted -- fall back to the end
        // rather than dropping it.
        let placedIDs = Set(result.map(\.0.id))
        result += ghosts.filter { !placedIDs.contains($0.id) }
            .map { ($0, !revivedIDs.contains($0.id)) }
        return result
    }

    private var rowsAreaHeight: CGFloat {
        CGFloat(min(displayRows.count, PanelMetrics.maxVisibleRows)) * PanelMetrics.rowHeight
    }

    private var contentHeight: CGFloat {
        PanelMetrics.inputHeight + (displayRows.isEmpty ? 0 : rowsAreaHeight)
    }

    private func addTask() {
        model.dispatch(.add(title: input, after: nil))
        input = ""
    }

    private func toggle(id: String) {
        focus = .row(id)
        let isGhost = ghosts.contains { $0.id == id }
        let isCompleted = isGhost && !revivedIDs.contains(id)

        if isCompleted {
            // Currently completed (whether freshly so or still lingering):
            // un-complete it. Stays in `ghosts`, just marked as reviving,
            // until `model.snapshot` confirms it's active again -- see
            // `revivedIDs`.
            ghostRemovalTasks[id]?.cancel()
            ghostRemovalTasks[id] = nil
            revivedIDs.insert(id)
            model.dispatch(.setDone(id: id, done: false))
        } else if isGhost {
            // Was reviving (still in `ghosts`, not yet confirmed active) --
            // re-completing it before that confirmation lands just flips it
            // back and restarts the lingering timer.
            revivedIDs.remove(id)
            model.dispatch(.setDone(id: id, done: true))
            scheduleGhostRemoval(id: id)
        } else if let rows = model.snapshot?.rows, let idx = rows.firstIndex(where: { $0.id == id }) {
            model.dispatch(.setDone(id: id, done: true))
            ghostAnchors[id] = idx > 0 ? rows[idx - 1].id : ""
            ghosts.append(rows[idx])
            scheduleGhostRemoval(id: id)
        }
    }

    private func scheduleGhostRemoval(id: String) {
        ghostRemovalTasks[id] = Task {
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled else { return }
            removeGhost(id: id)
        }
    }

    /// Called once a ghost's lingering window actually elapses (not on
    /// revive, which goes through `toggle` above). If it still held focus,
    /// hands focus to the row that was after it, rather than letting it
    /// drop to nil the moment its row disappears from the tree.
    private func removeGhost(id: String) {
        var successor: FocusTarget?
        if focus == .row(id) {
            let chain = focusChain
            if let idx = chain.firstIndex(of: .row(id)) {
                successor = chain[(idx + 1) % chain.count]
            }
        }
        ghosts.removeAll { $0.id == id }
        ghostAnchors[id] = nil
        ghostRemovalTasks[id] = nil
        revivedIDs.remove(id)
        if let successor {
            focus = successor
        }
    }

    /// Drops any reviving ghost once `model.snapshot` actually includes it
    /// again, now that the live active list can supply it -- see
    /// `revivedIDs`.
    private func reconcileRevivedGhosts() {
        guard !revivedIDs.isEmpty, let rows = model.snapshot?.rows else { return }
        let confirmed = revivedIDs.intersection(rows.map(\.id))
        guard !confirmed.isEmpty else { return }
        for id in confirmed {
            ghosts.removeAll { $0.id == id }
            ghostAnchors[id] = nil
        }
        revivedIDs.subtract(confirmed)
    }
}

private struct CaptureTaskRow: SwiftUI.View {
    let row: TaskRow
    let completed: Bool
    let isFocused: Bool
    let isEditing: Bool
    @Binding var editText: String
    var focus: FocusState<CaptureView.FocusTarget?>.Binding
    let onToggle: () -> Void
    let onFocusRequest: () -> Void
    let onCommitEdit: () -> Void
    let onCancelEdit: () -> Void

    var body: some SwiftUI.View {
        HStack(spacing: 10) {
            Button(action: onToggle) {
                Image(systemName: completed ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(completed ? Color.secondary : Color.accentColor)
            }
            .buttonStyle(.plain)
            // Not independently tab-stoppable: the row itself (via
            // .focusable() at the call site) is the single focus target,
            // so this doesn't compete with it for Tab/alt+j/alt+k.
            .focusable(false)
            // Toggling completion mid-edit would immediately ghost the row
            // whose title is being typed into -- block it until the edit
            // finishes.
            .disabled(isEditing)

            if isEditing {
                TextField("", text: $editText)
                    .textFieldStyle(.plain)
                    .focused(focus, equals: .editingRow(row.id))
                    .onSubmit(onCommitEdit)
                    .onExitCommand(perform: onCancelEdit)
            } else {
                Text(row.title)
                    .strikethrough(completed)
                    .foregroundStyle(completed ? .secondary : .primary)
            }

            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .background(isFocused ? Color.accentColor.opacity(0.15) : Color.clear)
        .contentShape(Rectangle())
        .onTapGesture(perform: onFocusRequest)
        .animation(.easeOut(duration: 0.2), value: completed)
    }
}
