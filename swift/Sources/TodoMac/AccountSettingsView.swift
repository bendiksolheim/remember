import SwiftUI
import TodoKit

/// Signing in, so todos sync between devices. Called "Account", not "Sync":
/// the user signs in to an account, and syncing is just what it gets them.
/// Syncing itself runs on its own in the background, so there is nothing
/// to start by hand here, only status.
struct AccountSettingsView: SwiftUI.View {
    @Environment(TodoModel.self) private var model

    var body: some SwiftUI.View {
        Form {
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
        .formStyle(.grouped)
    }
}

private struct SignedInSection: SwiftUI.View {
    @Environment(TodoModel.self) private var model
    @State private var isConfirmingSignOut = false

    var body: some SwiftUI.View {
        Section {
            LabeledContent("Signed in as") {
                Text(model.accountEmail ?? "Your account")
                    .textSelection(.enabled)
            }
            LabeledContent("Last synced") {
                if let lastSyncedAt = model.lastSyncedAt {
                    // Re-rendered every so often so "2 minutes ago" doesn't
                    // freeze while the window stays open.
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
        } header: {
            Text("Account")
        } footer: {
            Text("Your todos sync in the background while Todo is running.")
                .foregroundStyle(.secondary)
        }

        Section {
            Button("Sign Out…") { isConfirmingSignOut = true }
        }
        .alert("Sign out?", isPresented: $isConfirmingSignOut) {
            Button("Sign Out", role: .destructive) { model.signOut() }
            Button("Cancel", role: .cancel) {}
        } message: {
            // Signing in to another account later resets local data (see
            // `App::bind_sync_account`); this is the moment that's set up.
            Text("Your todos stay on this Mac but stop syncing. If you later sign in to a different account, the todos on this Mac are replaced by that account's, and changes that haven't synced yet are lost.")
        }
    }
}

/// One form with two modes. Signing in is the common case, so it's the
/// default; creating an account is one click away.
private struct SignInSection: SwiftUI.View {
    @Environment(TodoModel.self) private var model

    @State private var isCreatingAccount = false
    @State private var email = ""
    @State private var password = ""
    @State private var errorMessage: String?
    @State private var errorDetail: String?
    @State private var isSubmitting = false

    var body: some SwiftUI.View {
        Section {
            TextField("Email", text: $email)
                .textContentType(.username)
            SecureField("Password", text: $password)
                .textContentType(isCreatingAccount ? .newPassword : .password)
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

            HStack {
                Button(isCreatingAccount
                       ? "Already have an account? Sign in"
                       : "No account? Create one") {
                    isCreatingAccount.toggle()
                    errorMessage = nil
                    errorDetail = nil
                }
                .buttonStyle(.link)

                Spacer()

                if isSubmitting {
                    ProgressView().controlSize(.small)
                }
                Button(isCreatingAccount ? "Create Account" : "Sign In", action: submit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(isSubmitting || email.isEmpty || password.isEmpty)
            }
        } header: {
            VStack(alignment: .leading, spacing: 6) {
                Text(isCreatingAccount ? "Create an Account" : "Use Todo on All Your Devices")
                    .font(.headline)
                Text("Your todos are stored on this Mac. Sign in to get them on your other devices too. An account is optional: everything works without one.")
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.bottom, 4)
        } footer: {
            if isCreatingAccount {
                Text("Use at least 6 characters for the password.")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func submit() {
        guard !isSubmitting, !email.isEmpty, !password.isEmpty else { return }
        let creating = isCreatingAccount
        errorMessage = nil
        errorDetail = nil
        isSubmitting = true
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
