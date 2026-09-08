import AppKit
import Foundation

private struct WindowState: Codable {
    let id: UInt32
    let moveSequence: UInt64
    let wheelSequence: UInt64
    let lastX: Double?
    let lastY: Double?
    let lastDeltaX: Double?
    let lastDeltaY: Double?
    let lastTimestamp: Double?
}

private struct FixtureState: Codable {
    let schema: Int
    let role: String
    let pid: Int32
    let keyWindow: UInt32?
    let windows: [String: WindowState]
}

private final class ProbeView: NSView {
    let label: String
    private let publish: () -> Void
    var moveSequence: UInt64 = 0
    var wheelSequence: UInt64 = 0
    var lastLocation: NSPoint?
    var lastDelta: NSPoint?
    var lastTimestamp: TimeInterval?

    init(label: String, frame: NSRect, publish: @escaping () -> Void) {
        self.label = label
        self.publish = publish
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = label == "A"
            ? NSColor.systemBlue.withAlphaComponent(0.18).cgColor
            : NSColor.systemGreen.withAlphaComponent(0.18).cgColor
    }

    required init?(coder: NSCoder) { nil }
    override var acceptsFirstResponder: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(
            rect: bounds,
            options: [.activeAlways, .mouseMoved, .mouseEnteredAndExited],
            owner: self,
            userInfo: nil
        ))
    }

    private func recordMove(_ event: NSEvent) {
        moveSequence &+= 1
        lastLocation = convert(event.locationInWindow, from: nil)
        lastTimestamp = event.timestamp
        publish()
    }

    override func mouseMoved(with event: NSEvent) { recordMove(event) }
    override func mouseEntered(with event: NSEvent) { recordMove(event) }
    override func mouseExited(with event: NSEvent) { recordMove(event) }

    override func scrollWheel(with event: NSEvent) {
        wheelSequence &+= 1
        lastLocation = convert(event.locationInWindow, from: nil)
        lastDelta = NSPoint(x: event.scrollingDeltaX, y: event.scrollingDeltaY)
        lastTimestamp = event.timestamp
        publish()
    }
}

private final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let role: String
    private let stateURL: URL
    private let commandURL: URL?
    private var windows: [String: NSWindow] = [:]
    private var views: [String: ProbeView] = [:]
    private var timer: Timer?

    init(role: String, stateURL: URL, commandURL: URL?) {
        self.role = role
        self.stateURL = stateURL
        self.commandURL = commandURL
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        if role == "target" {
            makeWindow(label: "A", origin: NSPoint(x: 80, y: 160))
            makeWindow(label: "B", origin: NSPoint(x: 390, y: 160))
        } else {
            makeWindow(label: "guard", origin: NSPoint(x: 720, y: 160))
        }
        NSApp.activate(ignoringOtherApps: true)
        windows[role == "target" ? "A" : "guard"]?.makeKeyAndOrderFront(nil)
        publish()
        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            self?.pollCommand()
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.invalidate()
    }

    private func makeWindow(label: String, origin: NSPoint) {
        let frame = NSRect(origin: origin, size: NSSize(width: 260, height: 180))
        let window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "AgenTerm SkyLight Fixture \(label)"
        window.isReleasedWhenClosed = false
        window.acceptsMouseMovedEvents = true
        window.delegate = self
        let view = ProbeView(label: label, frame: NSRect(origin: .zero, size: frame.size)) { [weak self] in
            self?.publish()
        }
        window.contentView = view
        windows[label] = window
        views[label] = view
        window.orderFront(nil)
    }

    func windowWillClose(_ notification: Notification) { publish() }
    func windowDidBecomeKey(_ notification: Notification) { publish() }
    func windowDidResignKey(_ notification: Notification) { publish() }

    private func pollCommand() {
        guard let commandURL,
              let data = try? Data(contentsOf: commandURL),
              let command = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !command.isEmpty else { return }
        try? FileManager.default.removeItem(at: commandURL)
        if command == "close-B" {
            windows["B"]?.close()
            publish()
        } else if command == "quit" {
            NSApp.terminate(nil)
        }
    }

    private func publish() {
        var rows: [String: WindowState] = [:]
        for (label, window) in windows where window.isVisible {
            guard let view = views[label] else { continue }
            rows[label] = WindowState(
                id: UInt32(window.windowNumber),
                moveSequence: view.moveSequence,
                wheelSequence: view.wheelSequence,
                lastX: view.lastLocation.map { Double($0.x) },
                lastY: view.lastLocation.map { Double($0.y) },
                lastDeltaX: view.lastDelta.map { Double($0.x) },
                lastDeltaY: view.lastDelta.map { Double($0.y) },
                lastTimestamp: view.lastTimestamp
            )
        }
        let state = FixtureState(
            schema: 1,
            role: role,
            pid: getpid(),
            keyWindow: NSApp.keyWindow.map { UInt32($0.windowNumber) },
            windows: rows
        )
        guard let data = try? JSONEncoder().encode(state) else { return }
        try? data.write(to: stateURL, options: .atomic)
    }
}

private func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else {
        return nil
    }
    return CommandLine.arguments[index + 1]
}

guard let role = argument("--role"), ["target", "guard"].contains(role),
      let state = argument("--state") else {
    fputs("usage: Fixture --role target|guard --state PATH [--command PATH]\n", stderr)
    exit(2)
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
private let delegate = AppDelegate(
    role: role,
    stateURL: URL(fileURLWithPath: state),
    commandURL: argument("--command").map { URL(fileURLWithPath: $0) }
)
app.delegate = delegate
app.run()
