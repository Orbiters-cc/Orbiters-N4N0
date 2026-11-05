// A stand-in for the Unity editor on the Mac, for tests/mac_stand_ins.rs: an application with a window, started with
// -projectPath, working from the project's folder and holding Temp/UnityLockfile. It answers a request to quit as Unity
// does: with Temp/modified in the project it asks a question first, in a modal alert it answers by itself after a moment
// (Temp/answer-cancel makes that answer Cancel and it stays open), else it quits. With Temp/fail-at-once it ends at once.
import AppKit

let args = CommandLine.arguments
guard let flag = args.firstIndex(where: { $0.lowercased() == "-projectpath" }), flag + 1 < args.count else { exit(2) }
let project = args[flag + 1]
let temp = project + "/Temp/"
func has(_ name: String) -> Bool { FileManager.default.fileExists(atPath: temp + name) }

FileManager.default.changeCurrentDirectoryPath(project)
try? FileManager.default.createDirectory(atPath: temp, withIntermediateDirectories: true)
if has("fail-at-once") { exit(3) }
let lock = open(temp + "UnityLockfile", O_RDWR | O_CREAT, 0o644)
if flock(lock, LOCK_EX | LOCK_NB) != 0 { exit(9) }
try? args.joined(separator: "\n").write(toFile: temp + "started-with", atomically: true, encoding: .utf8)

final class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow!
    func applicationDidFinishLaunching(_ note: Notification) {
        window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 500, height: 300), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "\((project as NSString).lastPathComponent) - Main - macOS - Unity"
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard has("modified") else { return .terminateNow }
        let alert = NSAlert()
        alert.messageText = "Scene(s) Have Been Modified"
        for title in ["Save", "Don't Save", "Cancel"] { alert.addButton(withTitle: title) }
        let answer: NSApplication.ModalResponse = has("answer-cancel") ? .alertThirdButtonReturn : .alertSecondButtonReturn
        let timer = Timer(timeInterval: 3.0, repeats: false) { _ in NSApp.stopModal(withCode: answer) }
        RunLoop.current.add(timer, forMode: .modalPanel)
        let response = alert.runModal()
        alert.window.orderOut(nil)
        return response == .alertThirdButtonReturn ? .terminateCancel : .terminateNow
    }
}
let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
