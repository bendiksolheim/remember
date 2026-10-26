import UIKit

extension Notification.Name {
    /// Posted when the user shakes the device -- iOS's undo gesture.
    /// SwiftUI has no shake event of its own.
    static let rememberDeviceDidShake = Notification.Name("rememberDeviceDidShake")
}

extension UIWindow {
    open override func motionEnded(_ motion: UIEvent.EventSubtype, with event: UIEvent?) {
        if motion == .motionShake {
            NotificationCenter.default.post(name: .rememberDeviceDidShake, object: nil)
        }
        super.motionEnded(motion, with: event)
    }
}
