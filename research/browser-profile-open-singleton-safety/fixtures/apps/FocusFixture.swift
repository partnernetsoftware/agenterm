import AppKit
import Darwin
import Dispatch
import Foundation

private let stateSchema = "agenterm.singleton-safety.focus-fixture-state/v1"
private let primaryBundleID = "com.partnernetsoftware.agenterm.singleton-safety.primary"
private let secondaryBundleID = "com.partnernetsoftware.agenterm.singleton-safety.secondary"
private let maximumPathBytes = 4096
private let maximumCommandBytes = 64

private struct Configuration {
    let role: String
    let stateURL: URL
    let commandURL: URL
}

private func failUsage(_ code: String) -> Never {
    fputs("focus-fixture: \(code)\n", stderr)
    exit(2)
}

private func parseArguments() -> Configuration {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.count == 6 else { failUsage("invalid_arguments") }

    var values: [String: String] = [:]
    var index = 0
    while index < arguments.count {
        let key = arguments[index]
        guard ["--role", "--state", "--command"].contains(key), values[key] == nil else {
            failUsage("invalid_arguments")
        }
        let value = arguments[index + 1]
        guard !value.isEmpty, value.utf8.count <= maximumPathBytes else {
            failUsage("argument_out_of_range")
        }
        values[key] = value
        index += 2
    }

    guard let role = values["--role"], role == "primary" || role == "secondary",
          let statePath = values["--state"], let commandPath = values["--command"] else {
        failUsage("invalid_arguments")
    }
    return Configuration(
        role: role,
        stateURL: URL(fileURLWithPath: statePath),
        commandURL: URL(fileURLWithPath: commandPath)
    )
}

private final class FixtureDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private let configuration: Configuration
    private let bundleID: String
    private var window: NSWindow?
    private var timer: Timer?
    private var signalSource: DispatchSourceSignal?
    private var sequence: UInt64 = 0
    private var terminating = false
    private var rejectedCommandData: Data?
    private var rejectedCommandFailureCode: String?

    init(configuration: Configuration, bundleID: String) {
        self.configuration = configuration
        self.bundleID = bundleID
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        installTerminationHandler()
        let frame = NSRect(x: 240, y: 220, width: 360, height: 220)
        let window = NSWindow(
            contentRect: frame,
            styleMask: [.titled, .closable],
            backing: .buffered,
            defer: false
        )
        window.title = "AgenTerm Singleton Focus Fixture"
        window.isReleasedWhenClosed = false
        window.delegate = self
        self.window = window

        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        publish(event: "ready")

        timer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { [weak self] _ in
            self?.pollCommand()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func windowWillClose(_ notification: Notification) {
        guard !terminating else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, !self.terminating else { return }
            self.publish(event: "window-closed")
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        timer?.invalidate()
        signalSource?.cancel()
        if !terminating {
            terminating = true
            publish(event: "terminating")
        }
    }

    private func installTerminationHandler() {
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { [weak self] in
            self?.requestTermination()
        }
        source.resume()
        signalSource = source
    }

    private func requestTermination() {
        guard !terminating else { return }
        terminating = true
        publish(event: "terminating")
        NSApp.terminate(nil)
    }

    private func pollCommand() {
        let path = configuration.commandURL.path
        guard FileManager.default.fileExists(atPath: path) else { return }
        guard let handle = FileHandle(forReadingAtPath: path) else {
            rejectCommand(data: nil, failureCode: "command_open_failed")
            return
        }

        let data: Data
        do {
            data = try handle.read(upToCount: maximumCommandBytes + 1) ?? Data()
            try handle.close()
        } catch {
            rejectCommand(data: nil, failureCode: "command_read_failed")
            return
        }

        guard data.count <= maximumCommandBytes else {
            rejectCommand(data: data, failureCode: "command_invalid")
            return
        }
        let command: String
        if data == Data("close-window\n".utf8) {
            command = "close-window"
        } else if data == Data("quit\n".utf8) {
            command = "quit"
        } else {
            rejectCommand(data: data, failureCode: "command_invalid")
            return
        }
        if command == "close-window" {
            guard let window, window.isVisible else {
                rejectCommand(data: data, failureCode: "window_already_closed")
                return
            }
        }
        do {
            try FileManager.default.removeItem(at: configuration.commandURL)
        } catch {
            rejectCommand(data: data, failureCode: "command_remove_failed")
            return
        }
        rejectedCommandData = nil
        rejectedCommandFailureCode = nil
        if command == "close-window" {
            window?.close()
        } else {
            requestTermination()
        }
    }

    private func rejectCommand(data: Data?, failureCode: String) {
        guard rejectedCommandData != data || rejectedCommandFailureCode != failureCode else { return }
        rejectedCommandData = data
        rejectedCommandFailureCode = failureCode
        publish(event: "command-rejected", failureCode: failureCode)
    }

    private func writeStateAtomically(_ data: Data) throws {
        let destination = configuration.stateURL
        let temporary = URL(fileURLWithPath: destination.path + ".tmp-\(getpid())-\(sequence)")
        // The cleanup is registered only after this invocation's own write succeeds, so a
        // pre-existing sibling temporary (a protocol failure owned by someone else) is never
        // removed by this process.
        try data.write(to: temporary, options: .withoutOverwriting)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let handle = try FileHandle(forWritingTo: temporary)
        try handle.synchronize()
        try handle.close()
        guard rename(temporary.path, destination.path) == 0 else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
        let readback = try Data(contentsOf: destination)
        guard readback == data else {
            throw NSError(domain: "agenterm.singleton-safety.focus-fixture", code: 1)
        }
    }

    private func publish(event: String, failureCode: String? = nil) {
        sequence &+= 1
        let visible = window?.isVisible == true
        let number = visible ? (window?.windowNumber ?? 0) : 0
        if visible && (number <= 0 || UInt64(number) > UInt64(UInt32.max)) { _exit(3) }
        let handle: Any = visible ? number : NSNull()
        let state: [String: Any] = [
            "schema": stateSchema,
            "sequence": sequence,
            "event": event,
            "role": configuration.role,
            "pid": Int(getpid()),
            "bundle_id": bundleID,
            "window": [
                "handle": handle,
                "ordinary_onscreen_count": visible ? 1 : 0,
            ],
            "failure_code": failureCode ?? NSNull(),
        ]
        do {
            let data = try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
            try writeStateAtomically(data)
        } catch {
            _exit(3)
        }
    }
}

private let configuration = parseArguments()
private let expectedBundleID = configuration.role == "primary" ? primaryBundleID : secondaryBundleID
guard let actualBundleID = Bundle.main.bundleIdentifier, actualBundleID == expectedBundleID else {
    fputs("focus-fixture: bundle_identity_mismatch\n", stderr)
    exit(2)
}

private let application = NSApplication.shared
application.setActivationPolicy(.regular)
private let delegate = FixtureDelegate(configuration: configuration, bundleID: actualBundleID)
application.delegate = delegate
application.run()
