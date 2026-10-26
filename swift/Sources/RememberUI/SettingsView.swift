import SwiftUI
import RememberKit

/// The gear's sheet: lists and account -- the iPhone version of the Mac's
/// Settings window (`ListsSettingsView`, `AccountSettingsView`). Lists come
/// first: they're what's changed most often.
struct SettingsView: SwiftUI.View {
    /// Set when opened from the "+" pill: the cursor starts in "New List".
    let focusNewList: Bool
    @Environment(RememberModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some SwiftUI.View {
        NavigationStack {
            Form {
                ListsSection(focusNewList: focusNewList)
                if model.isSignedIn {
                    SignedInSection()
                } else {
                    SignInSection()
                }
                if let keychainError = model.keychainError {
                    Section {
                        Label(keychainError, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}

// MARK: - Lists

private struct ListsSection: SwiftUI.View {
    let focusNewList: Bool
    @Environment(RememberModel.self) private var model
    @State private var newListName = ""
    @FocusState private var isNewListFocused: Bool

    private var lists: [ListRow] { model.snapshot?.lists ?? [] }

    var body: some SwiftUI.View {
        Section {
            ForEach(lists, id: \.id) { list in
                ListSettingsRow(list: list, canDelete: lists.count > 1)
            }
            .onMove(perform: move)

            HStack(spacing: 12) {
                Image(systemName: "plus.circle.fill")
                    .foregroundStyle(Color.accentColor)
                TextField("New List", text: $newListName)
                    .focused($isNewListFocused)
                    .submitLabel(.done)
                    .onSubmit(addList)
            }
        } header: {
            Text("Lists")
        } footer: {
            Text("Your lists appear as buttons under the text field. Tap a name to rename it, hold and drag to move it, and swipe left to delete it.")
        }
        .task {
            guard focusNewList else { return }
            // A field in a sheet that's still sliding up doesn't take focus.
            try? await Task.sleep(for: .milliseconds(450))
            isNewListFocused = true
        }
    }

    private func addList() {
        guard !newListName.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        model.dispatch(.addList(name: newListName, after: lists.last?.id))
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
/// undo step and sync op. Same as the Mac's `ListSettingsRow`.
private struct ListSettingsRow: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    let list: ListRow
    let canDelete: Bool

    @State private var name: String
    @State private var isConfirmingDelete = false
    @FocusState private var isEditing: Bool

    init(list: ListRow, canDelete: Bool) {
        self.list = list
        self.canDelete = canDelete
        _name = State(initialValue: list.name)
    }

    var body: some SwiftUI.View {
        HStack(spacing: 12) {
            Circle()
                .fill(color(for: list.color))
                .frame(width: 12, height: 12)
            TextField("List Name", text: $name)
                .focused($isEditing)
                .submitLabel(.done)
                .onSubmit(commitRename)
            // Open tasks, as on the main screen. Deleting (below) is about
            // `taskCount`, done ones included.
            Text("\(list.activeCount)")
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .accessibilityLabel(list.activeCount == 1 ? "1 open task" : "\(list.activeCount) open tasks")
        }
        .swipeActions(edge: .trailing) {
            if canDelete {
                Button(role: .destructive) {
                    // An empty list has nothing to lose, so no question.
                    if list.taskCount == 0 {
                        model.dispatch(.deleteList(id: list.id))
                    } else {
                        isConfirmingDelete = true
                    }
                } label: {
                    Label("Delete", systemImage: "trash")
                }
            }
        }
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

// MARK: - Account

private struct SignedInSection: SwiftUI.View {
    @Environment(RememberModel.self) private var model
    @State private var isConfirmingSignOut = false

    var body: some SwiftUI.View {
        Section {
            LabeledContent("Signed in as", value: model.accountEmail ?? "Your account")
            LabeledContent("Last synced") {
                if let lastSyncedAt = model.lastSyncedAt {
                    // Re-rendered every so often so "2 minutes ago" doesn't
                    // freeze while the sheet stays open.
                    TimelineView(.periodic(from: .now, by: 30)) { _ in
                        Text(lastSyncedAt.formatted(.relative(presentation: .named)))
                    }
                } else {
                    Text("Not yet")
                }
            }
            if let lastSyncError = model.lastSyncError {
                Label {
                    Text("The last sync failed: \(lastSyncError)")
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                }
            }
            if model.isMissingChanges {
                Label {
                    Text("Some changes from another device can't show here yet, because that device hasn't sent everything they build on. Updating Remember on all your devices resends it.")
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.yellow)
                }
            }
            Button("Sign Out…", role: .destructive) { isConfirmingSignOut = true }
        } header: {
            Text("Account")
        } footer: {
            Text("Your tasks sync while Remember is open, and right after you leave it.")
        }
        .alert("Sign out?", isPresented: $isConfirmingSignOut) {
            Button("Sign Out", role: .destructive) { model.signOut() }
            Button("Cancel", role: .cancel) {}
        } message: {
            // Signing in to another account later resets local data (see
            // `App::bind_sync_account`); this is the moment that's set up.
            Text("Your tasks stay on this iPhone but stop syncing. If you later sign in to a different account, the tasks on this iPhone are replaced by that account's, and changes that haven't synced yet are lost.")
        }
    }
}

/// One form with two modes. Signing in is the common case, so it's the
/// default; creating an account is one tap away. Same as the Mac's.
private struct SignInSection: SwiftUI.View {
    @Environment(RememberModel.self) private var model

    @State private var isCreatingAccount = false
    @State private var email = ""
    @State private var password = ""
    @State private var errorMessage: String?
    @State private var errorDetail: String?
    @State private var isSubmitting = false
    @FocusState private var focusedField: Field?

    private enum Field { case email, password }

    var body: some SwiftUI.View {
        Section {
            TextField("Email", text: $email)
                .textContentType(.username)
                .keyboardType(.emailAddress)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .submitLabel(.next)
                .focused($focusedField, equals: .email)
                .onSubmit { focusedField = .password }
            SecureField("Password", text: $password)
                .textContentType(isCreatingAccount ? .newPassword : .password)
                .submitLabel(.go)
                .focused($focusedField, equals: .password)
                .onSubmit(submit)

            if let errorMessage {
                VStack(alignment: .leading, spacing: 4) {
                    Text(errorMessage)
                        .foregroundStyle(.red)
                    if let errorDetail {
                        Text(errorDetail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                    }
                }
            }

            Button(action: submit) {
                HStack {
                    Text(isCreatingAccount ? "Create Account" : "Sign In")
                    Spacer()
                    if isSubmitting {
                        ProgressView()
                    }
                }
            }
            .disabled(isSubmitting || email.isEmpty || password.isEmpty)

            Button(isCreatingAccount
                   ? "Already have an account? Sign in"
                   : "No account? Create one") {
                isCreatingAccount.toggle()
                errorMessage = nil
                errorDetail = nil
            }
            .font(.footnote)
        } header: {
            Text(isCreatingAccount ? "Create an Account" : "Use Remember on All Your Devices")
        } footer: {
            Text(isCreatingAccount
                 ? "Use at least 6 characters for the password."
                 : "Your tasks are stored on this iPhone. Sign in to get them on your other devices too. An account is optional: everything works without one.")
        }
    }

    private func submit() {
        guard !isSubmitting, !email.isEmpty, !password.isEmpty else { return }
        let creating = isCreatingAccount
        errorMessage = nil
        errorDetail = nil
        isSubmitting = true
        focusedField = nil
        Task {
            do {
                if creating {
                    try await model.signUp(email: email, password: password)
                } else {
                    try await model.signIn(email: email, password: password)
                }
                password = ""
            } catch {
                // The server's answer is technical (e.g. "HTTP 400"), so it
                // goes underneath a plain explanation rather than instead
                // of one.
                errorMessage = creating
                    ? "Couldn't create the account."
                    : "Couldn't sign in. Check your email and password, and that you're online."
                errorDetail = "\(error)"
            }
            isSubmitting = false
        }
    }
}
