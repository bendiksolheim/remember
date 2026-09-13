import Cocoa
import SwiftUI

@MainActor
protocol SpotlightPanelDelegate: AnyObject {
    /// Fired when the panel loses key status for any reason other than us
    /// hiding it ourselves -- i.e. the user clicked into another app.
    func spotlightPanelDidResignKey(_ panel: SpotlightPanel)
}

/// Panel sizing shared with `CaptureView`, which lays out its content to
/// these same dimensions -- kept in one place so the two can't drift apart.
/// `rowHeight` is a best estimate of `CaptureTaskRow`'s rendered height
/// (20pt padding + ~20pt text line); it's cosmetic and safe to tune by eye
/// once this is actually running, since nothing here can be measured
/// without a compiler.
enum PanelMetrics {
    static let width: CGFloat = 680
    static let inputHeight: CGFloat = 68
    static let rowHeight: CGFloat = 40
    static let maxVisibleRows = 8
}

/// A borderless, floating, all-Spaces panel styled after Spotlight's own
/// window: translucent vibrancy background, no title bar, no Dock
/// presence. `.nonactivatingPanel` means showing or clicking the panel
/// doesn't activate the app or deactivate whatever app was previously
/// frontmost -- that's what lets focus land back there automatically once
/// the panel is dismissed, with no manual restoration needed. It does not,
/// by itself, allow the panel to become key, so `canBecomeKey` is
/// overridden below -- otherwise its `TextField` could never get real
/// keyboard focus.
@MainActor
final class SpotlightPanel: NSPanel {
    weak var spotlightDelegate: SpotlightPanelDelegate?

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }

    init<Content: View>(content: Content) {
        let width: CGFloat = PanelMetrics.width
        let height: CGFloat = PanelMetrics.inputHeight

        super.init(
            contentRect: NSRect(x: 0, y: 0, width: width, height: height),
            styleMask: [.borderless, .fullSizeContentView, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )

        isFloatingPanel = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        titleVisibility = .hidden
        titlebarAppearsTransparent = true
        isMovableByWindowBackground = false
        hidesOnDeactivate = false

        let effect = NSVisualEffectView()
        effect.material = .hudWindow
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = 14
        effect.layer?.masksToBounds = true

        let hosting = NSHostingView(rootView: AnyView(content))
        hosting.translatesAutoresizingMaskIntoConstraints = false

        effect.addSubview(hosting)
        NSLayoutConstraint.activate([
            hosting.leadingAnchor.constraint(equalTo: effect.leadingAnchor),
            hosting.trailingAnchor.constraint(equalTo: effect.trailingAnchor),
            hosting.topAnchor.constraint(equalTo: effect.topAnchor),
            hosting.bottomAnchor.constraint(equalTo: effect.bottomAnchor),
        ])

        contentView = effect
    }

    override func resignKey() {
        super.resignKey()
        spotlightDelegate?.spotlightPanelDidResignKey(self)
    }

    /// Horizontally centered, top edge pinned to the top-third line of
    /// whichever screen currently has the mouse cursor -- matches where
    /// Spotlight itself appears, rather than always the main/menu-bar
    /// screen. Anchoring the *top* edge (rather than centering the whole
    /// frame) matters once `resize(toContentHeight:)` starts changing the
    /// height: the panel should grow downward from a fixed input-field
    /// position, not expand symmetrically around a midpoint.
    func positionOnActiveScreen() {
        let mouseLocation = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { NSMouseInRect(mouseLocation, $0.frame, false) }
            ?? NSScreen.main
        guard let screen else { return }

        let visible = screen.visibleFrame
        let topY = visible.minY + visible.height * 0.66
        let origin = NSPoint(x: visible.midX - frame.width / 2, y: topY - frame.height)
        setFrameOrigin(origin)
    }

    /// Resizes to fit `height` (as computed by `CaptureView` from its
    /// current row count), keeping the *top* edge fixed so the panel grows
    /// downward -- the input field never moves, only the list below it
    /// expands or contracts. Safe to call while hidden (e.g. a task gets
    /// added/completed while the panel is ordered out): it'll already be
    /// the right size the next time the hotkey shows it.
    func resize(toContentHeight height: CGFloat) {
        let newHeight = max(height, PanelMetrics.inputHeight)
        guard abs(newHeight - frame.height) > 0.5 else { return }

        let topY = frame.maxY
        let newFrame = NSRect(x: frame.minX, y: topY - newHeight, width: frame.width, height: newHeight)
        setFrame(newFrame, display: isVisible, animate: isVisible)
    }
}
