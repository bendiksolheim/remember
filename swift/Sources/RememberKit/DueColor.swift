import SwiftUI

/// Color for a due-date chip, combining `state` with whether the task is
/// done. Done always reads as neutral regardless of `state` -- a finished
/// task's due date is no longer a call to action, overdue or not. Shared by
/// `RememberMac`'s `CaptureTaskRow` and `RememberUI`'s `TaskRowView` so a due date
/// looks the same wherever it's shown.
public func dueChipColor(state: DueState, done: Bool) -> Color {
    guard !done else { return .secondary }
    switch state {
    case .overdue: return .red
    case .today: return .orange
    case .later, .none: return .secondary
    }
}
