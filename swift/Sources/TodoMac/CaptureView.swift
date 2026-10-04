import AppKit
import SwiftUI
import TodoKit

// Same qualification convention as TodoUI (see TaskRowView.swift): TodoKit's
// generated `App`/`View` collide by name with `SwiftUI.App`/`SwiftUI.View`
// once both modules are imported, so conformances/opaque returns are
// qualified explicitly rather than trusting contextual resolution --
// unverified since there's no compiler in this container.

/// The entire macOS UI for v1: a quick-add field, the live incomplete task
/// list, inline title editing ("e" on the focused row), reordering the
/// focused row (alt+shift+j/alt+shift+k), switching the sticky current list
/// (`ListPillRow`), and moving the focused row to a different list
/// (⌘⌥1–⌘⌥9, mirroring the plain ⌘1–⌘9 switch shortcuts -- see
/// `moveFocusedRow`). Deleting is still out of scope here -- that remains
/// reachable only via `TodoUI`'s `TaskListView`, which iOS still uses but
/// this target no longer wires up.
struct CaptureView: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    var onDismiss: () -> Void
    /// Opens the Settings window on its Lists tab — wired to the pill row's
    /// trailing "+" pill. `CaptureView` has no window of its own to show
    /// Settings from (see `SpotlightPanel`'s doc comment), so this, like
    /// `onDismiss`, is owned and supplied by `AppDelegate`.
    var onOpenListsSettings: () -> Void
    /// Called with the panel's required total height whenever the row
    /// count changes, so `SpotlightPanel` can resize the actual window --
    /// a plain SwiftUI `ScrollView` has no natural "hug my content up to a
    /// max, then scroll" sizing behavior, so height is computed here from
    /// `displayRows.count` instead of trusting layout to report it.
    var onContentHeightChange: (CGFloat) -> Void

    @State private var input = ""
    /// Live result of `TodoModel.detectDue` for `input`, re-checked on every
    /// keystroke. `nil` means no recognized trailing phrase.
    @State private var dueDetection: DueDetection?
    /// Set when the user clicks the badge to reject a detection for this
    /// input — cleared again on the next keystroke, so a fresh phrase is
    /// never silently suppressed by an old dismissal.
    @State private var dueDismissed = false
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
    /// Ids of rows mid-fade-out after being moved to a different list by
    /// `moveFocusedRow`. Unlike `ghosts`, these are never revivable by
    /// clicking -- a list-move isn't a toggle -- so they get their own
    /// smaller set of state rather than overloading the completion-ghost
    /// machinery with a second, differently-behaved kind of entry.
    @State private var fadingMoveIDs: Set<String> = []
    /// The row's data at the moment it started fading, keyed by id -- it's
    /// about to drop out of `model.snapshot.rows` (it now belongs to a
    /// different list), so `displayRows` can no longer source it from
    /// there once that snapshot lands.
    @State private var fadingMoveRows: [String: TaskRow] = [:]
    /// Where each fading-move row was, same convention as `ghostAnchors`:
    /// the id it directly followed when the move started, or `""` if it
    /// was first.
    @State private var fadingMoveAnchors: [String: String] = [:]
    /// Pending final-removal task per fading-move row id, cancelled by
    /// `reconcileUndoneMoves` if ⌘Z brings the row back before the fade
    /// finishes.
    @State private var fadingMoveTasks: [String: Task<Void, Never>] = [:]
    /// Id of the list whose pill in `ListPillRow` should flash, set by
    /// `blink(listId:)` right after a move lands and cleared ~0.5s later.
    @State private var blinkingListID: String?
    /// Local monitor for alt+j/alt+k: SwiftUI's `.keyboardShortcut` (used
    /// below for Undo/Redo/Toggle) isn't honored while the quick-add
    /// TextField is first responder for a bare-Option combo -- unlike
    /// Command-based shortcuts, Option+letter is normally live text input
    /// (it types accented/special characters), so AppKit lets the text
    /// field consume it before falling back to key-equivalent scanning. A
    /// local monitor intercepts before that happens, regardless of focus.
    @State private var optionNavMonitor: Any?
    /// Measured height of `ListPillRow`, fed into `contentHeight` so the
    /// panel grows when the roster wraps to a second line. Seeded to a
    /// single-row estimate (not 0) so the panel doesn't visibly start too
    /// short and then jump once the real `GeometryReader` measurement
    /// lands a frame later.
    @State private var pillRowHeight: CGFloat = 38

    var body: some SwiftUI.View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                TextField("Add a task…", text: $input)
                    .textFieldStyle(.plain)
                    .font(.system(size: 22))
                    .focused($focus, equals: .input)
                    .onSubmit(addTask)
                    .onChange(of: input) { _, newValue in
                        dueDetection = newValue.isEmpty ? nil : model.detectDue(newValue)
                        dueDismissed = false
                    }
                if let detection = dueDetection, !dueDismissed {
                    CaptureDueBadge(label: detection.label) { dueDismissed = true }
                }
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)

            Divider()
            ListPillRow(onOpenListsSettings: onOpenListsSettings, blinkingListID: blinkingListID)
                .padding(.horizontal, 20)
                .padding(.vertical, 8)
                .background(
                    GeometryReader { geo in
                        Color.clear.preference(key: PillRowHeightKey.self, value: geo.size.height)
                    }
                )
                .onPreferenceChange(PillRowHeightKey.self) { pillRowHeight = $0 }

            if !displayRows.isEmpty {
                Divider()
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(displayRows, id: \.row.id) { entry in
                            CaptureTaskRow(
                                row: entry.row,
                                completed: entry.completed,
                                fading: entry.fading,
                                isFocused: focus == .row(entry.row.id),
                                isEditing: editingRowID == entry.row.id,
                                editText: $editText,
                                focus: $focus,
                                onToggle: { toggle(id: entry.row.id) },
                                onFocusRequest: { focus = .row(entry.row.id) },
                                onCommitEdit: commitEdit,
                                onCancelEdit: cancelEdit
                            )
                            // A fading-move row is inert -- see
                            // `fadingMoveIDs`'s doc comment -- so it's
                            // dropped from the Tab/alt+j/alt+k chain here,
                            // same as `focusChain` below already excludes it.
                            .focusable(!entry.fading)
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
        // `SpotlightPanel`'s `.hudWindow` material always renders dark,
        // regardless of the system's actual light/dark setting -- without
        // this, `.primary`/`.secondary`/`accentColor` would keep following
        // system appearance and render light-mode (dark) text on that
        // always-dark background in Light Mode, a real contrast bug rather
        // than just a styling choice.
        .preferredColorScheme(.dark)
        .onExitCommand(perform: onDismiss)
        .onAppear {
            focus = .input
            installOptionNavMonitor()
        }
        .onDisappear(perform: removeOptionNavMonitor)
        .onChange(of: contentHeight, initial: true) { _, newValue in
            onContentHeightChange(newValue)
        }
        .onChange(of: model.snapshot?.revision) { _, _ in
            reconcileRevivedGhosts()
            reconcileUndoneMoves()
        }
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
                // ⌘0 jumps to All, ⌘1–⌘9 to the first 9 lists in sidebar
                // order — the keyboard half of the pill row below, so
                // switching lists never requires reaching for the mouse.
                // All gets the least reachable digit deliberately: it's
                // the least-used list once the user has their own lists
                // set up, so it shouldn't squat on the easiest shortcut.
                // Lives here (not inside `ListPillRow`) to keep every
                // global shortcut owned by this one overlay, same as
                // Undo/Redo/Toggle/Edit above.
                Button("Select All List") { model.setCurrentList(.all) }
                    .keyboardShortcut("0", modifiers: .command)
                ForEach(Array((model.snapshot?.lists ?? []).prefix(9).enumerated()), id: \.element.id) { index, list in
                    Button("Select \(list.name) List") { model.setCurrentList(.list(id: list.id)) }
                        .keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: .command)
                }
                // ⌘⌥1–⌘⌥9: move the focused row into one of the same first
                // 9 lists ⌘1–⌘9 switch to -- deliberately the same digit
                // mapping, Option added to mean "send it there" instead of
                // "go there". Not Shift: ⌘⇧3/4/5 (and ⌘⇧6 on Touch Bar
                // Macs) are macOS's own screenshot shortcuts system-wide,
                // and partially shadowing only *some* digits in one
                // mnemonic set would be worse than using a different
                // modifier for all of them. There's no ⌘⌥0: a task can't
                // be moved into a filter that isn't a real list, so that
                // digit is simply never bound to a move button at all.
                ForEach(Array((model.snapshot?.lists ?? []).prefix(9).enumerated()), id: \.element.id) { index, list in
                    Button("Move Focused to \(list.name) List") { moveFocusedRow(to: list.id) }
                        .keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: [.command, .option])
                }
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
            let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
            guard flags == [.option] || flags == [.option, .shift] else { return event }
            // `charactersIgnoringModifiers` honors Shift (only Option is
            // stripped), so alt+shift+j arrives as "J" -- lowercase first to
            // key the switch on the letter alone.
            let isReorder = flags.contains(.shift)
            switch event.charactersIgnoringModifiers?.lowercased() {
            case "j":
                // Still swallowed (returns nil) while editing, not just
                // skipped -- letting the event through would have AppKit
                // insert its dead-key/accented character into the field
                // being edited instead of doing nothing.
                if editingRowID == nil {
                    isReorder ? moveFocusedRow(by: 1) : moveFocus(by: 1)
                }
                return nil
            case "k":
                if editingRowID == nil {
                    isReorder ? moveFocusedRow(by: -1) : moveFocus(by: -1)
                }
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
    /// every displayed row (completion ghosts included, fading-move rows
    /// excluded -- see `fadingMoveIDs`) in display order. Both ends wrap
    /// around.
    private var focusChain: [FocusTarget] {
        [.input] + displayRows.filter { !$0.fading }.map { .row($0.row.id) }
    }

    private func moveFocus(by delta: Int) {
        let chain = focusChain
        guard !chain.isEmpty else { return }
        let currentIndex = focus.flatMap { chain.firstIndex(of: $0) } ?? 0
        let nextIndex = (currentIndex + delta + chain.count) % chain.count
        focus = chain[nextIndex]
    }

    /// alt+shift+j/alt+shift+k: swap the focused row with its adjacent
    /// *active* neighbor (ghosts and the input field aren't real order
    /// entries, so the `.row` match plus `rows.firstIndex` already excludes
    /// them -- no separate ghost/input check needed). Clamps at the ends
    /// rather than wrapping like `moveFocus` does: sending a row to the
    /// opposite end of the list on an edge press would be a much bigger,
    /// easier-to-regret jump than just moving focus there.
    ///
    /// `Move`'s `after` is relational (todo-core resolves it directly
    /// against its own order, same as the CLI's `mv` does against a
    /// snapshot) -- here that means "the id `rows` already puts right where
    /// the focused row should land": moving down lands right after the
    /// neighbor being passed, moving up lands right after whatever was two
    /// slots up (or at the front, if the neighbor was already first).
    private func moveFocusedRow(by delta: Int) {
        guard case .row(let id)? = focus,
            let rows = model.snapshot?.rows,
            let index = rows.firstIndex(where: { $0.id == id })
        else { return }
        let neighborIndex = index + delta
        guard rows.indices.contains(neighborIndex) else { return }

        let after: String?
        if delta > 0 {
            after = rows[neighborIndex].id
        } else {
            after = neighborIndex > 0 ? rows[neighborIndex - 1].id : nil
        }
        model.dispatch(.move(id: id, after: after))
    }

    private func toggleFocusedRow() {
        guard case .row(let id)? = focus else { return }
        toggle(id: id)
    }

    /// ⌘⌥1–⌘⌥9: moves the focused row into `listId` (always one of the
    /// first 9 lists -- see the overlay buttons above). Silently does
    /// nothing if there's no eligible focused row at all (no focus, mid
    /// rename, or a completion ghost -- renaming/reviving and moving don't
    /// mix, same reasoning as `canEditFocusedRow`) or if the row is already
    /// in `listId` -- no command, no fade, no blink, since nothing would
    /// actually change.
    private func moveFocusedRow(to listId: String) {
        guard case .row(let id)? = focus,
            editingRowID == nil,
            !ghosts.contains(where: { $0.id == id }),
            let rows = model.snapshot?.rows,
            let index = rows.firstIndex(where: { $0.id == id }),
            rows[index].listId != listId
        else { return }
        let row = rows[index]

        // Computed from the chain *before* dispatch -- once the row leaves
        // `model.snapshot.rows` the chain would no longer contain it at
        // all, so there'd be nothing to find "the next one after" from.
        // Unlike `removeGhost`'s equivalent check, no `focus == .row(id)`
        // re-check is needed before applying it: everything here runs
        // synchronously, so focus can't have moved out from under us
        // between the guard above and this assignment.
        let chain = focusChain
        guard let chainIndex = chain.firstIndex(of: .row(id)) else { return }
        let successor = chain[(chainIndex + 1) % chain.count]

        model.dispatch(.setList(id: id, listId: listId))
        focus = successor

        // Under "All", the row never actually leaves the displayed set --
        // it's still there, just recategorized -- so fading it out would
        // look like it's being removed and then mysteriously isn't. Only
        // fade when the current filter is a single list the row is truly
        // about to drop out of.
        var isAllView = false
        if case .all = model.snapshot?.currentList { isAllView = true }
        if !isAllView {
            withAnimation(.easeOut(duration: 0.5)) {
                fadingMoveAnchors[id] = index > 0 ? rows[index - 1].id : ""
                fadingMoveRows[id] = row
                fadingMoveIDs.insert(id)
            }
            scheduleMoveFadeRemoval(id: id)
        }

        blink(listId: listId)
    }

    private func scheduleMoveFadeRemoval(id: String) {
        fadingMoveTasks[id] = Task {
            try? await Task.sleep(for: .seconds(0.5))
            guard !Task.isCancelled else { return }
            fadingMoveRows.removeValue(forKey: id)
            fadingMoveAnchors.removeValue(forKey: id)
            fadingMoveTasks[id] = nil
            fadingMoveIDs.remove(id)
        }
    }

    /// Flashes `listId`'s pill in `ListPillRow` briefly -- the move's only
    /// feedback when the row itself doesn't visibly disappear (the "All"
    /// case above). Animated on both ends: a quick flash in, then an
    /// explicit fade back out, rather than just snapping the highlight away
    /// after the delay.
    private func blink(listId: String) {
        withAnimation(.easeInOut(duration: 0.15)) {
            blinkingListID = listId
        }
        Task {
            try? await Task.sleep(for: .seconds(0.5))
            guard blinkingListID == listId else { return }
            withAnimation(.easeOut(duration: 0.3)) {
                blinkingListID = nil
            }
        }
    }

    /// Cancels a fading move and snaps the row straight back to normal,
    /// deliberately un-animated, the instant `model.snapshot` shows it back
    /// in the current list -- i.e. ⌘Z undid the move while the 0.5s fade
    /// was still playing. Undo reflects true state immediately everywhere
    /// else in this view; a move's exit animation shouldn't be the one
    /// exception that lags behind it.
    private func reconcileUndoneMoves() {
        guard !fadingMoveIDs.isEmpty, let rows = model.snapshot?.rows else { return }
        let backInView = fadingMoveIDs.intersection(Set(rows.map(\.id)))
        guard !backInView.isEmpty else { return }
        var transaction = Transaction()
        transaction.disablesAnimations = true
        withTransaction(transaction) {
            for id in backInView {
                fadingMoveTasks[id]?.cancel()
                fadingMoveTasks[id] = nil
                fadingMoveRows.removeValue(forKey: id)
                fadingMoveAnchors.removeValue(forKey: id)
                fadingMoveIDs.remove(id)
            }
        }
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

    /// Active rows in their natural order, with each completion ghost and
    /// each fading-move row reinserted right after the anchor it was
    /// captured with -- never at the front -- so a row's index never
    /// changes across a complete/un-complete cycle or a move's fade-out.
    /// Any id still in `ghosts` or `fadingMoveIDs` is excluded from `active`
    /// unconditionally (not just while genuinely completed/fading): while
    /// reviving a ghost, or right after a move dispatches, the row is still
    /// in the overlay set *and* may briefly also still be in the stale
    /// snapshot, and without this filter it would render twice for a frame.
    private var displayRows: [(row: TaskRow, completed: Bool, fading: Bool)] {
        let overlayIDs = Set(ghosts.map(\.id)).union(fadingMoveIDs)
        let active = (model.snapshot?.rows ?? []).filter { !overlayIDs.contains($0.id) }

        func overlaysAnchored(to anchor: String) -> [(TaskRow, Bool, Bool)] {
            let ghostEntries = ghosts.filter { ghostAnchors[$0.id] == anchor }
                .map { ($0, !revivedIDs.contains($0.id), false) }
            let fadingEntries = fadingMoveIDs.filter { fadingMoveAnchors[$0] == anchor }
                .compactMap { id in fadingMoveRows[id].map { (row: $0, completed: false, fading: true) } }
            return ghostEntries + fadingEntries
        }

        var result = overlaysAnchored(to: "")
        for row in active {
            result.append((row, false, false))
            result += overlaysAnchored(to: row.id)
        }
        // An overlay row whose anchor is itself gone (e.g. also completed,
        // also moved, or deleted) has nowhere to be reinserted -- fall back
        // to the end rather than dropping it.
        let placedIDs = Set(result.map(\.0.id))
        result += ghosts.filter { !placedIDs.contains($0.id) }
            .map { ($0, !revivedIDs.contains($0.id), false) }
        result += fadingMoveIDs.filter { !placedIDs.contains($0) }
            .compactMap { id in fadingMoveRows[id].map { (row: $0, completed: false, fading: true) } }
        return result
    }

    private var rowsAreaHeight: CGFloat {
        CGFloat(min(displayRows.count, PanelMetrics.maxVisibleRows)) * PanelMetrics.rowHeight
    }

    private var contentHeight: CGFloat {
        PanelMetrics.inputHeight + pillRowHeight + (displayRows.isEmpty ? 0 : rowsAreaHeight)
    }

    private func addTask() {
        if let detection = dueDetection, !dueDismissed {
            model.dispatch(.add(title: detection.strippedTitle, after: nil, due: detection.due, listId: nil))
        } else {
            model.dispatch(.add(title: input, after: nil, due: nil, listId: nil))
        }
        input = ""
        dueDetection = nil
        dueDismissed = false
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

/// Row of list pills under the capture panel's input field -- this target's
/// only way to change which list a capture lands in, since it has no other
/// window (see `SettingsWindowController`'s Lists pane, reachable via the
/// trailing "+" pill, for creating/renaming/deleting lists instead).
/// Replaces the old flat `Menu`-based switcher: every list is a visible,
/// one-click pill instead of being hidden until opened, and ⌘0–⌘9 (wired in
/// `CaptureView`'s overlay above) switch lists without touching the mouse
/// at all -- each pill that has one shows its own "⌘n" on the left so the
/// mapping never has to be memorized or counted out by eye as the roster
/// grows (only the first 9 lists get one; see `pill`'s `shortcutDigit`).
/// "All" is pinned first and isn't reorderable, deliberately sitting on the
/// least reachable digit (⌘0) since it's the least-used list once the user
/// has their own lists set up; the rest follow `snapshot.lists`' sidebar
/// order, which `MoveList`/`ListsSettingsView` already manage. A small dot
/// marks whichever list is the actual capture destination when it differs
/// from the one being viewed -- i.e. while viewing "All", since picking a
/// concrete list always makes it both (see `TodoModel.setCurrentList`'s own
/// doc comment on why "All" doesn't change the sticky destination).
/// `blinkingListID`, set by `CaptureView.blink`, briefly highlights
/// whichever pill just received a task moved into it by ⌘⌥1–⌘⌥9 -- the only
/// feedback for that move while viewing "All", where the moved row doesn't
/// otherwise visibly disappear.
private struct ListPillRow: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    var onOpenListsSettings: () -> Void
    var blinkingListID: String?

    var body: some SwiftUI.View {
        FlowLayout(spacing: 6) {
            pill(
                label: "All",
                shortcutDigit: 0,
                isSelected: isAll,
                showsCaptureDot: false,
                isBlinking: false,
                color: .accentColor,
                textColor: readableTextColor(on: .accentColor)
            ) {
                model.setCurrentList(.all)
            }
            ForEach(Array((model.snapshot?.lists ?? []).enumerated()), id: \.element.id) { index, list in
                pill(
                    label: list.name,
                    // Only the first 9 lists have a ⌘-shortcut at all (⌘1–⌘9
                    // — see the hidden buttons in `CaptureView`'s overlay);
                    // nil here just omits the hint, it never misrepresents
                    // one that doesn't exist.
                    shortcutDigit: index < 9 ? index + 1 : nil,
                    isSelected: isSelected(list.id),
                    showsCaptureDot: isAll && model.snapshot?.captureListId == list.id,
                    isBlinking: list.id == blinkingListID,
                    color: color(for: list.color),
                    textColor: pillTextColor(for: list.color)
                ) {
                    model.setCurrentList(.list(id: list.id))
                }
            }
            Button(action: onOpenListsSettings) {
                Image(systemName: "plus")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 20, height: 20)
                    .background(Color.secondary.opacity(0.12), in: Circle())
            }
            .buttonStyle(.plain)
            .focusable(false)
        }
    }

    private var isAll: Bool {
        guard let snapshot = model.snapshot else { return true }
        if case .all = snapshot.currentList { return true }
        return false
    }

    private func isSelected(_ id: String) -> Bool {
        guard let snapshot = model.snapshot, case .list(let current) = snapshot.currentList else { return false }
        return current == id
    }

    /// `color` is this pill's identity hue (a real list's `ListColor`, or
    /// `.accentColor` for "All" -- every pill has one now, there's no more
    /// neutral/colorless case). `textColor` is what the solid-fill states
    /// below use, since hue-matched text on a hue-matched fill reads as
    /// low-contrast no matter how the opacity is tuned -- callers pass
    /// `pillTextColor(for:)` for a real list, or `readableTextColor(on:)` for
    /// `.accentColor`, which isn't one of the 9 fixed `ListColor` cases and
    /// can't be hardcoded (it's the user's macOS system accent color).
    ///
    /// Resting and selected are no longer the same hue at different
    /// opacities: resting is an outline on a clear fill (hue-on-near-neutral,
    /// same pattern `DueColor` chips already use successfully), selected
    /// flips to a solid fill. `isBlinking` forces the solid fill even over a
    /// merely-resting pill, plus an extra white ring, so it stays the single
    /// most intense tier above selected -- the flash is a momentary
    /// "something just landed here" signal and needs to read as such even on
    /// the pill you're currently viewing.
    @ViewBuilder
    private func pill(
        label: String,
        shortcutDigit: Int?,
        isSelected: Bool,
        showsCaptureDot: Bool,
        isBlinking: Bool,
        color: Color,
        textColor: Color,
        action: @escaping () -> Void
    ) -> some SwiftUI.View {
        let solid = isSelected || isBlinking
        let foreground = solid ? textColor : color

        Button(action: action) {
            HStack(spacing: 4) {
                if let shortcutDigit {
                    Text("⌘\(shortcutDigit)")
                        .font(.system(size: 10, weight: .medium, design: .rounded))
                        .foregroundStyle(foreground.opacity(0.7))
                }
                Text(label)
                    .font(.system(size: 12, weight: .medium))
                if showsCaptureDot {
                    Circle()
                        .fill(color)
                        .frame(width: 5, height: 5)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(solid ? color : Color.clear, in: Capsule())
            .overlay {
                if isBlinking {
                    Capsule().strokeBorder(Color.white.opacity(0.9), lineWidth: 2)
                } else if !solid {
                    Capsule().strokeBorder(color, lineWidth: 1)
                }
            }
            .foregroundStyle(foreground)
        }
        .buttonStyle(.plain)
        // Click/⌘-shortcut only, like the old Menu-based switcher -- not
        // part of the alt+j/alt+k row-focus chain or Tab order.
        .focusable(false)
    }
}

/// Readable text color for a solid pill fill whose color isn't one of the 9
/// fixed `ListColor` cases -- i.e. `.accentColor`, which is a user-level
/// macOS System Settings choice (blue, graphite, yellow, ...) and so can't be
/// hardcoded the way `pillTextColor(for:)` hardcodes the known `ListColor`
/// set. AppKit-only (`NSColor`), which is why this lives here rather than
/// alongside `pillTextColor(for:)` in the cross-platform `TodoKit`.
private func readableTextColor(on color: Color) -> Color {
    let rgb = NSColor(color).usingColorSpace(.deviceRGB) ?? NSColor(color)
    var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0
    rgb.getRed(&r, green: &g, blue: &b, alpha: nil)
    let luminance = 0.299 * r + 0.587 * g + 0.114 * b
    return luminance > 0.6 ? .black : .white
}

/// Left-to-right, top-to-bottom wrapping layout -- SwiftUI has no built-in
/// equivalent. Used only by `ListPillRow` so a wide roster of lists wraps
/// onto additional lines instead of being clipped or forcing horizontal
/// scroll, matching how the task list below grows the whole panel's height
/// for more content rather than confining it to a fixed box.
private struct FlowLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var rowWidth: CGFloat = 0
        var totalHeight: CGFloat = 0
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if rowWidth > 0, rowWidth + spacing + size.width > maxWidth {
                totalHeight += rowHeight + spacing
                rowWidth = 0
                rowHeight = 0
            }
            rowWidth += (rowWidth > 0 ? spacing : 0) + size.width
            rowHeight = max(rowHeight, size.height)
        }
        totalHeight += rowHeight
        return CGSize(width: maxWidth.isFinite ? maxWidth : rowWidth, height: totalHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x: CGFloat = bounds.minX
        var y: CGFloat = bounds.minY
        var rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x > bounds.minX, x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

private struct PillRowHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) {
        value = nextValue()
    }
}

private struct CaptureTaskRow: SwiftUI.View {
    let row: TaskRow
    let completed: Bool
    /// True while this row is fading out after being moved to a different
    /// list (see `CaptureView.fadingMoveIDs`). Purely visual -- the row is
    /// already gone from the live data, this is its exit animation -- so
    /// it's rendered dimmed and made non-interactive rather than, say,
    /// still toggleable like a completion ghost is.
    let fading: Bool
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

            if let label = row.dueLabel, !isEditing {
                CaptureDueChip(label: label, tint: dueChipColor(state: row.dueState, done: completed))
            }
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 8)
        .background(isFocused ? Color.accentColor.opacity(0.25) : Color.clear)
        .overlay(alignment: .leading) {
            // A flat background tint alone washes out too easily on this
            // translucent HUD panel -- the leading bar is what actually
            // reads at a glance as "keyboard commands act on this row".
            if isFocused {
                Rectangle().fill(Color.accentColor).frame(width: 3)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture(perform: onFocusRequest)
        .animation(.easeOut(duration: 0.2), value: completed)
        // Done rows recede further than just their secondary/strikethrough
        // text color -- fading and completed are mutually exclusive (a row
        // is never both an overlay-list-move and a completion ghost at
        // once), so this one modifier can serve both.
        .opacity(fading ? 0.3 : (completed ? 0.55 : 1))
        // Inert, not just dimmed: it's already gone from the list this
        // panel is showing, so clicking it (toggle, focus, edit) shouldn't
        // do anything until it's fully removed.
        .allowsHitTesting(!fading)
    }
}

/// The visual due-date chip — calendar icon on a capsule background. Used
/// as plain decoration on task rows, and (wrapped in `CaptureDueBadge`
/// below) as the clickable quick-add detection indicator, so a due date
/// looks the same wherever it's shown.
private struct CaptureDueChip: SwiftUI.View {
    let label: String
    var tint: Color = .accentColor

    var body: some SwiftUI.View {
        Label(label, systemImage: "calendar")
            .font(.caption)
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(tint.opacity(0.2), in: Capsule())
    }
}

/// A clickable `CaptureDueChip` showing a live-detected due date, for the
/// quick-add field. Clicking it calls `onDismiss` — the caller is
/// responsible for clearing the detection state that produced `label`.
private struct CaptureDueBadge: SwiftUI.View {
    let label: String
    let onDismiss: () -> Void

    var body: some SwiftUI.View {
        Button(action: onDismiss) {
            CaptureDueChip(label: label)
        }
        .buttonStyle(.plain)
        // Not part of the alt+j/alt+k row-focus chain or Tab order — it's a
        // click-only escape hatch, not a navigable control.
        .focusable(false)
    }
}
