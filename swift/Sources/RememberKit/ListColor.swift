import SwiftUI

/// Maps a list's persisted `ListColor` slot to an actual `Color`. The
/// mapping lives here, in the UI layer, not in `remember-core` — `ListColor` is
/// an opaque palette slot rather than a stored hex value specifically so
/// re-theming this mapping never touches stored data (see `ListColor`'s own
/// doc comment in `remember-core`). Shared by `RememberMac` (`ListPillRow`,
/// `ListsSettingsView`) and `RememberUI`/iOS so a list's color means the same
/// thing everywhere it's used.
public func color(for listColor: ListColor) -> Color {
    switch listColor {
    case .blue: return .blue
    case .purple: return .purple
    case .pink: return .pink
    case .orange: return .orange
    case .teal: return .teal
    case .indigo: return .indigo
    case .mint: return .mint
    case .yellow: return .yellow
    case .cyan: return .cyan
    }
}

/// Readable text color for a pill rendered with a *solid* `color(for:)` fill
/// (`ListPillRow`'s selected/blinking states). Picked once per case from each
/// system color's own dark-mode luminance (`0.299R + 0.587G + 0.114B`), not
/// computed at runtime: the 9 cases are a closed set, so there's no need to
/// pull in color-space conversion for a value that never changes. If Apple
/// ever redefines one of these dynamic system colors' dark-mode RGB, this
/// table is what would need rechecking.
public func pillTextColor(for listColor: ListColor) -> Color {
    switch listColor {
    case .blue, .purple, .pink, .indigo: return .white
    case .orange, .teal, .mint, .yellow, .cyan: return .black
    }
}
