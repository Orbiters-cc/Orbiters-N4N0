// A stand-in for Blender on the Mac, for tests/mac_stand_ins.rs: an application that notes the file the system asks it to
// open (in the file named by its bundle's Contents/Resources/note-path) and quits.
import AppKit

final class Delegate: NSObject, NSApplicationDelegate {
    func application(_ sender: NSApplication, openFile filename: String) -> Bool {
        let notePath = Bundle.main.bundleURL.appendingPathComponent("Contents/Resources/note-path")
        if let target = try? String(contentsOf: notePath, encoding: .utf8) {
            try? filename.write(toFile: target.trimmingCharacters(in: .whitespacesAndNewlines), atomically: true, encoding: .utf8)
        }
        NSApp.terminate(nil)
        return true
    }
}
let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.prohibited)
app.run()
