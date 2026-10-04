import Foundation
import Observation

public func defaultDatabasePath() -> String {
    let dir = FileManager.default
        .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("no.bendik.todo", isDirectory: true)
    try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.appendingPathComponent("todo.sqlite3").path
}

@MainActor
@Observable
public final class TodoModel {
    public private(set) var snapshot: Snapshot?
    public private(set) var isSyncing = false
    public private(set) var lastSyncError: String?
    // A stored property, not computed from `KeychainStore.load()` on every
    // access — `@Observable` only tracks reads/writes of stored properties,
    // so a computed one reading external state wouldn't invalidate SwiftUI
    // views when sign-in/out actually changes it.
    public private(set) var isSignedIn: Bool
    private let app: App
    private var bridge: ListenerBridge?
    // `nil` when no Supabase project is configured — sync is opt-in, so the
    // app must work fully with no backend wired up at all.
    private let syncClient: SyncClient?
    // Kept alive for the lifetime of `TodoModel`, not created fresh per
    // call like `syncNow`'s: `startAutoSync` is only ever called once, so
    // this is the one listener its coordinator reports through for as long
    // as the app runs.
    private var autoSyncBridge: SyncStatusBridge?

    public init(
        dbPath: String = defaultDatabasePath(),
        supabaseURL: String = "",
        supabaseAnonKey: String = ""
    ) throws {
        // `open` collides with Swift's `open` access-level keyword, so the
        // generated binding escapes it with backticks — confirmed by
        // reading the actual generated todo_ffi.swift, not assumed.
        self.app = try App.`open`(dbPath: dbPath)

        // `syncClient` is a `let`, so — unlike the `var` properties below,
        // which get an implicit `nil` default — it must be assigned before
        // `self` is captured by the closure that follows, or definite
        // initialization fails.
        self.syncClient = supabaseURL.isEmpty
            ? nil
            : SyncClient(supabaseUrl: supabaseURL, anonKey: supabaseAnonKey)
        self.isSignedIn = KeychainStore.load() != nil

        let bridge = ListenerBridge { [weak self] snap in
            Task { @MainActor in self?.snapshot = snap }
        }
        self.bridge = bridge
        app.subscribe(listener: bridge)

        if let syncClient {
            let autoSyncBridge = SyncStatusBridge(
                onComplete: { [weak self] _ in
                    Task { @MainActor in self?.lastSyncError = nil }
                },
                onError: { [weak self] error in
                    Task { @MainActor in self?.lastSyncError = "\(error)" }
                },
                onSessionRefreshed: { [weak self] session in
                    Task { @MainActor in self?.persist(session) }
                }
            )
            self.autoSyncBridge = autoSyncBridge
            syncClient.startAutoSync(app: self.app, listener: autoSyncBridge)
            // Picks up an existing session from a previous launch -- the
            // coordinator itself is what decides whether/when to actually
            // sync, this just tells it a token is available at all.
            syncClient.setSyncToken(session: loadSession())
        }
    }

    public func dispatch(_ command: Command) {
        do { try app.dispatch(command: command) }
        catch { print("dispatch failed: \(error)") }
    }

    public func setView(_ view: View) { app.setView(view: view) }

    /// Switches which list the snapshot reflects (`snapshot?.currentList`)
    /// and the one new captures land in — see
    /// `todo_core::App::set_current_list`'s own doc comment. Same
    /// swallow-and-log error handling as `dispatch`: a failed local write
    /// isn't something call sites should need a `do`/`catch` for.
    public func setCurrentList(_ listId: String) {
        do { try app.setCurrentList(listId: listId) }
        catch { print("setCurrentList failed: \(error)") }
    }

    public func flush() { try? app.flush() }

    /// Detects a due-date phrase ("today", "tomorrow", a weekday name, or
    /// "dec 25") at the end of `text`, for quick-add's live badge. `nil` if
    /// `text` doesn't end in a recognized phrase. Pure lookup — never
    /// dispatches anything itself.
    public func detectDue(_ text: String) -> DueDetection? {
        app.detectDue(text: text)
    }

    /// Sets the device's local UTC offset, used by both `detectDue` and
    /// every row's due label/overdue state. Call once at launch and again
    /// whenever it might have changed — app foreground, and the system
    /// timezone-change notification — since there's no way to observe this
    /// from the Rust side (see `App.setLocalOffsetSeconds`'s own doc
    /// comment for why).
    public func setLocalOffsetSeconds() {
        app.setLocalOffsetSeconds(offsetSeconds: Int32(TimeZone.current.secondsFromGMT()))
    }

    /// `async`, not a plain blocking call: `SyncClient.signUp`/`signIn` are
    /// synchronous `reqwest::blocking` calls under the hood — unlike
    /// `syncNow`, nothing on the Rust side moves them off-thread, so this
    /// hops to a detached task itself rather than freezing the caller (a
    /// button action, in practice) for the network round-trip.
    public func signUp(email: String, password: String) async throws {
        try await authenticate(email: email, password: password) { syncClient in
            try syncClient.signUp(email: email, password: password)
        }
    }

    public func signIn(email: String, password: String) async throws {
        try await authenticate(email: email, password: password) { syncClient in
            try syncClient.signIn(email: email, password: password)
        }
    }

    private func authenticate(
        email: String,
        password: String,
        _ call: @escaping @Sendable (SyncClient) throws -> Session
    ) async throws {
        guard let syncClient else { return }
        let session = try await Task.detached(priority: .userInitiated) {
            try call(syncClient)
        }.value
        persist(session)
        isSignedIn = true
        syncClient.setSyncToken(session: session)
        // A user might have used the app locally before ever signing in --
        // don't make them wait for the periodic fallback to find that out.
        syncClient.syncSoon()
    }

    /// Deliberately local-only: local data is kept exactly as is, this just
    /// forgets the credential so syncing stops until signed in again.
    public func signOut() {
        KeychainStore.delete()
        isSignedIn = false
        syncClient?.setSyncToken(session: nil)
    }

    /// Requests an immediate auto-sync attempt, bypassing the normal
    /// debounce/periodic wait — call this from platform lifecycle hooks
    /// (app foreground, the Mac waking from sleep). A no-op if sync isn't
    /// configured or the user isn't signed in.
    public func syncSoon() {
        syncClient?.syncSoon()
    }

    public func syncNow() {
        guard let syncClient, let session = loadSession() else { return }
        isSyncing = true
        lastSyncError = nil
        let statusBridge = SyncStatusBridge(
            onComplete: { [weak self] _ in
                Task { @MainActor in self?.isSyncing = false }
            },
            onError: { [weak self] error in
                Task { @MainActor in
                    self?.isSyncing = false
                    self?.lastSyncError = "\(error)"
                }
            },
            onSessionRefreshed: { [weak self] session in
                Task { @MainActor in self?.persist(session) }
            }
        )
        syncClient.syncNow(app: app, session: session, listener: statusBridge)
    }

    private func persist(_ session: Session) {
        let stored = StoredSession(
            accessToken: session.accessToken,
            refreshToken: session.refreshToken,
            userId: session.userId,
            expiresAt: session.expiresAt
        )
        if let data = try? JSONEncoder().encode(stored) {
            KeychainStore.save(data)
        }
    }

    private func loadSession() -> Session? {
        guard let data = KeychainStore.load(),
            let stored = try? JSONDecoder().decode(StoredSession.self, from: data)
        else { return nil }
        return Session(
            accessToken: stored.accessToken,
            refreshToken: stored.refreshToken,
            userId: stored.userId,
            expiresAt: stored.expiresAt
        )
    }
}

private final class ListenerBridge: SnapshotListener, @unchecked Sendable {
    private let handler: (Snapshot) -> Void
    init(_ handler: @escaping (Snapshot) -> Void) { self.handler = handler }
    func onChange(snapshot: Snapshot) { handler(snapshot) }
}

private final class SyncStatusBridge: SyncStatusListener, @unchecked Sendable {
    private let onComplete: (SyncOutcome) -> Void
    private let onError: (SyncError) -> Void
    private let onSessionRefreshed: (Session) -> Void

    init(
        onComplete: @escaping (SyncOutcome) -> Void,
        onError: @escaping (SyncError) -> Void,
        onSessionRefreshed: @escaping (Session) -> Void
    ) {
        self.onComplete = onComplete
        self.onError = onError
        self.onSessionRefreshed = onSessionRefreshed
    }

    func onSyncComplete(outcome: SyncOutcome) { onComplete(outcome) }
    func onSyncError(error: SyncError) { onError(error) }
    func onSessionRefreshed(session: Session) { onSessionRefreshed(session) }
}

/// Keychain can only store bytes — this is the plain data shape `Session`
/// (the uniffi-generated type) is marshaled through, nothing more.
private struct StoredSession: Codable {
    let accessToken: String
    let refreshToken: String
    let userId: String
    let expiresAt: Int64

    init(accessToken: String, refreshToken: String, userId: String, expiresAt: Int64) {
        self.accessToken = accessToken
        self.refreshToken = refreshToken
        self.userId = userId
        self.expiresAt = expiresAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        accessToken = try container.decode(String.self, forKey: .accessToken)
        refreshToken = try container.decode(String.self, forKey: .refreshToken)
        userId = try container.decode(String.self, forKey: .userId)
        // Missing in a session stored before refresh support shipped --
        // treat it as already-expired so the next sync attempt refreshes it
        // immediately rather than trusting a token whose real age is
        // unknown.
        expiresAt = try container.decodeIfPresent(Int64.self, forKey: .expiresAt) ?? 0
    }
}
