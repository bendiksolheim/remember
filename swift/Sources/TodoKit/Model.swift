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
    private let app: App
    private var bridge: ListenerBridge?

    public init(dbPath: String = defaultDatabasePath()) throws {
        // `open` collides with Swift's `open` access-level keyword, so the
        // generated binding escapes it with backticks — confirmed by
        // reading the actual generated todo_ffi.swift, not assumed.
        self.app = try App.`open`(dbPath: dbPath)
        let bridge = ListenerBridge { [weak self] snap in
            Task { @MainActor in self?.snapshot = snap }
        }
        self.bridge = bridge
        app.subscribe(listener: bridge)
    }

    public func dispatch(_ command: Command) {
        do { try app.dispatch(command: command) }
        catch { print("dispatch failed: \(error)") }
    }

    public func setView(_ view: View) { app.setView(view: view) }
    public func flush() { try? app.flush() }
}

private final class ListenerBridge: SnapshotListener, @unchecked Sendable {
    private let handler: (Snapshot) -> Void
    init(_ handler: @escaping (Snapshot) -> Void) { self.handler = handler }
    func onChange(snapshot: Snapshot) { handler(snapshot) }
}
