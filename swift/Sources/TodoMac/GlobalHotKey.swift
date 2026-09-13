import Carbon.HIToolbox
import Cocoa

/// Thin wrapper around the Carbon Event Manager's global-hotkey APIs --
/// the same mechanism Spotlight itself (and classic launchers) use to bind
/// a system-wide key combo. Chosen over `NSEvent.addGlobalMonitorForEvents`
/// because it never triggers the Accessibility permission prompt and can
/// register a modifier+key combo the OS itself routes to us, rather than
/// merely observing events other apps already received.
final class GlobalHotKey {
    // Carbon's callback is a plain C function pointer, which can never be
    // actor-isolated -- these are marked `nonisolated(unsafe)` rather than
    // hopping through an actor, on the invariant that Carbon always fires
    // this callback on the main run loop.
    private nonisolated(unsafe) static var handlers: [UInt32: () -> Void] = [:]
    private nonisolated(unsafe) static var nextID: UInt32 = 1
    private nonisolated(unsafe) static var installedHandler: EventHandlerRef?

    private let hotKeyID: UInt32
    private var hotKeyRef: EventHotKeyRef?

    /// `keyCode` is a Carbon virtual key code (see `CarbonKeyCode`), `modifiers`
    /// an OR of Carbon modifier masks (see `CarbonModifier`). Fails (returns
    /// nil) if the combo is already claimed by another app.
    init?(keyCode: UInt32, modifiers: UInt32, onPress: @escaping () -> Void) {
        GlobalHotKey.installEventHandlerOnce()

        let id = GlobalHotKey.nextID
        GlobalHotKey.nextID += 1

        var ref: EventHotKeyRef?
        let hotKeyIDStruct = EventHotKeyID(signature: OSType(bitPattern: 0x546F_646F /* 'Todo' */), id: id)
        let status = RegisterEventHotKey(
            keyCode,
            modifiers,
            hotKeyIDStruct,
            GetApplicationEventTarget(),
            0,
            &ref
        )
        guard status == noErr, let ref else { return nil }

        hotKeyID = id
        hotKeyRef = ref
        GlobalHotKey.handlers[id] = onPress
    }

    deinit {
        if let hotKeyRef {
            UnregisterEventHotKey(hotKeyRef)
        }
        GlobalHotKey.handlers[hotKeyID] = nil
    }

    /// Installs one process-wide Carbon event handler that dispatches to
    /// whichever `GlobalHotKey` instance registered the pressed combo's ID.
    /// Carbon callbacks are plain C function pointers (no captures allowed),
    /// so dispatch happens through the static `handlers` table rather than
    /// through instance state.
    private static func installEventHandlerOnce() {
        guard installedHandler == nil else { return }

        var eventType = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard),
            eventKind: UInt32(kEventHotKeyPressed)
        )

        InstallEventHandler(
            GetApplicationEventTarget(),
            { _, eventRef, _ -> OSStatus in
                guard let eventRef else { return -50 /* paramErr */ }
                var pressedID = EventHotKeyID()
                let status = GetEventParameter(
                    eventRef,
                    EventParamName(kEventParamDirectObject),
                    EventParamType(typeEventHotKeyID),
                    nil,
                    MemoryLayout<EventHotKeyID>.size,
                    nil,
                    &pressedID
                )
                guard status == noErr else { return status }
                GlobalHotKey.handlers[pressedID.id]?()
                return noErr
            },
            1,
            &eventType,
            nil,
            &installedHandler
        )
    }
}

/// Carbon virtual key codes needed for this app's fixed hotkey.
enum CarbonKeyCode {
    static let space: UInt32 = 49
}

/// Carbon modifier masks, combined with `|`.
enum CarbonModifier {
    static let option: UInt32 = UInt32(optionKey)
    static let command: UInt32 = UInt32(cmdKey)
}
