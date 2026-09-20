import SwiftUI
import TodoKit

/// The only UI surface for sync: sign in/up when signed out, sign out + a
/// manual "Sync Now" when signed in. Deliberately minimal — this is not the
/// place for account management polish, just enough to exercise the sync
/// backend end to end.
struct SyncSettingsView: SwiftUI.View {
    @Environment(TodoModel.self) private var model

    @State private var email = ""
    @State private var password = ""
    @State private var errorMessage: String?
    @State private var isSubmitting = false

    var body: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 16) {
            if model.isSignedIn {
                signedInSection
            } else {
                signedOutSection
            }
        }
        .padding(20)
        .frame(width: 320)
    }

    private var signedOutSection: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Sign in to sync").font(.headline)

            TextField("Email", text: $email)
                .textFieldStyle(.roundedBorder)
                .textContentType(.username)
            SecureField("Password", text: $password)
                .textFieldStyle(.roundedBorder)
                .textContentType(.password)
                .onSubmit { submit(isSignUp: false) }

            if let errorMessage {
                Text(errorMessage)
                    .foregroundStyle(.red)
                    .font(.caption)
            }

            HStack {
                if isSubmitting {
                    ProgressView().controlSize(.small)
                }
                Spacer()
                Button("Sign Up") { submit(isSignUp: true) }
                Button("Sign In") { submit(isSignUp: false) }
                    .keyboardShortcut(.defaultAction)
            }
            .disabled(isSubmitting || email.isEmpty || password.isEmpty)
        }
    }

    private var signedInSection: some SwiftUI.View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Signed in").font(.headline)

            if model.isSyncing {
                ProgressView("Syncing…").controlSize(.small)
            } else if let lastSyncError = model.lastSyncError {
                Text("Last sync failed: \(lastSyncError)")
                    .foregroundStyle(.red)
                    .font(.caption)
            }

            HStack {
                Button("Sync Now") { model.syncNow() }
                    .disabled(model.isSyncing)
                Spacer()
                Button("Sign Out", role: .destructive) { model.signOut() }
            }
        }
    }

    private func submit(isSignUp: Bool) {
        errorMessage = nil
        isSubmitting = true
        Task {
            do {
                if isSignUp {
                    try await model.signUp(email: email, password: password)
                } else {
                    try await model.signIn(email: email, password: password)
                }
                password = ""
            } catch {
                errorMessage = "\(error)"
            }
            isSubmitting = false
        }
    }
}
