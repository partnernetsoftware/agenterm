import AppKit
import CoreGraphics
import Darwin
import Foundation

private let requiredSymbols = [
    "SLEventPostToPid",
    "SLEventSetIntegerValueField",
    "CGEventSetWindowLocation",
]

private enum ProbeFailure: Error, CustomStringConvertible {
    case typed(String, String)
    var description: String {
        switch self { case let .typed(code, message): return "\(code): \(message)" }
    }
}

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

private struct HostState: Equatable, Codable {
    let pointerX: Double
    let pointerY: Double
    let frontmostPid: Int32
    let frontmostWindow: UInt32?
}

private final class SkyLight {
    typealias Post = @convention(c) (pid_t, UnsafeMutableRawPointer?) -> Void
    typealias SetInteger = @convention(c) (UnsafeMutableRawPointer?, UInt32, Int64) -> Void
    typealias SetLocation = @convention(c) (UnsafeMutableRawPointer?, Double, Double) -> Void

    private let handle: UnsafeMutableRawPointer?
    let post: Post?
    let setInteger: SetInteger?
    let setLocation: SetLocation?

    init(forceMissing: Bool = false) {
        guard !forceMissing else {
            handle = nil; post = nil; setInteger = nil; setLocation = nil
            return
        }
        let loaded = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW | RTLD_LOCAL)
        handle = loaded
        func resolve<T>(_ name: String, _: T.Type) -> T? {
            guard let loaded, let symbol = dlsym(loaded, name) else { return nil }
            // SAFETY: each symbol is converted only to its frozen C ABI shape;
            // the dlopen handle outlives every copied function pointer.
            return unsafeBitCast(symbol, to: T.self)
        }
        post = resolve(requiredSymbols[0], Post.self)
        setInteger = resolve(requiredSymbols[1], SetInteger.self)
        setLocation = resolve(requiredSymbols[2], SetLocation.self)
    }

    deinit { if let handle { dlclose(handle) } }
    var available: Bool { post != nil && setInteger != nil && setLocation != nil }
    var resolved: [String: Bool] {
        [requiredSymbols[0]: post != nil, requiredSymbols[1]: setInteger != nil, requiredSymbols[2]: setLocation != nil]
    }
}

private final class AttemptLedger {
    private(set) var count = 0
    func record() { count += 1 }
}

private func load(_ path: String) throws -> FixtureState {
    try JSONDecoder().decode(FixtureState.self, from: Data(contentsOf: URL(fileURLWithPath: path)))
}

private func waitState(_ path: String, deadline: Date, predicate: (FixtureState) -> Bool) throws -> FixtureState {
    while Date() < deadline {
        if let state = try? load(path), predicate(state) { return state }
        usleep(10_000)
    }
    throw ProbeFailure.typed("verification_timeout", "owned fixture did not publish the expected state")
}

private func windowRow(_ id: UInt32) -> [String: Any]? {
    let rows = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
    return rows.first { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value == id }
}

private func windowIsOnscreen(_ id: UInt32) -> Bool {
    guard let row = windowRow(id) else { return false }
    return (row[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true
}

private func exactWindow(id: UInt32, owner: pid_t) throws -> CGRect {
    guard id != 0, let row = windowRow(id) else {
        throw ProbeFailure.typed("stale_window", "the exact CGWindowID is absent")
    }
    guard (row[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == owner else {
        throw ProbeFailure.typed("window_owner_changed", "the exact CGWindowID owner does not match")
    }
    guard (row[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true,
          let raw = row[kCGWindowBounds as String] as? [String: CGFloat] else {
        throw ProbeFailure.typed("window_not_onscreen", "the exact CGWindowID is not an on-screen target")
    }
    let bounds = CGRect(x: raw["X"] ?? .nan, y: raw["Y"] ?? .nan,
                        width: raw["Width"] ?? .nan, height: raw["Height"] ?? .nan)
    guard bounds.width > 0, bounds.height > 0 else {
        throw ProbeFailure.typed("window_geometry_invalid", "the exact CGWindowID has invalid bounds")
    }
    return bounds
}

private func rejectAmbiguousOwnerTarget(owner: pid_t) throws {
    let rows = (CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? [])
        .filter {
            ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == owner &&
            ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0 &&
            ($0[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true
        }
    guard rows.count == 1 else {
        throw ProbeFailure.typed("window_ambiguous", "owner-only addressing resolved \(rows.count) windows")
    }
    throw ProbeFailure.typed("negative_accepted", "owner-only addressing unexpectedly became exact")
}

private func hostState() -> HostState {
    let pointer = NSEvent.mouseLocation
    let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] ?? []
    let first = rows.first {
        ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == frontmost &&
        ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0
    }
    return HostState(
        pointerX: pointer.x,
        pointerY: pointer.y,
        frontmostPid: frontmost,
        frontmostWindow: (first?[kCGWindowNumber as String] as? NSNumber)?.uint32Value
    )
}

private func stamp(_ event: CGEvent, sky: SkyLight, pid: pid_t, window: UInt32, local: CGPoint) throws {
    guard let setInteger = sky.setInteger, let setLocation = sky.setLocation else {
        throw ProbeFailure.typed("provider_unavailable", "required SkyLight symbols are missing")
    }
    let raw = Unmanaged.passUnretained(event).toOpaque()
    // SAFETY: the retained CGEvent owns this object for the complete synchronous
    // native call sequence; no pointer escapes or is stored by the probe.
    for (field, value): (UInt32, Int64) in [
        (0, 2), (1, 0), (3, 0), (7, 3), (40, Int64(pid)),
        (51, Int64(window)), (58, Int64(DispatchTime.now().uptimeNanoseconds % 1_000_000_000)),
        (91, Int64(window)), (92, Int64(window)),
    ] { setInteger(raw, field, value) }
    setLocation(raw, local.x, local.y)
}

private enum Action { case move, wheel(dx: Int32, dy: Int32) }

private func inject(
    sky: SkyLight, ledger: AttemptLedger, pid: pid_t, window: UInt32, action: Action
) throws {
    guard sky.available, let post = sky.post else {
        throw ProbeFailure.typed("provider_unavailable", "required SkyLight symbols are missing")
    }
    let bounds = try exactWindow(id: window, owner: pid)
    let local = CGPoint(x: bounds.width * 0.5, y: bounds.height * 0.5)
    let screen = CGPoint(x: bounds.minX + local.x, y: bounds.minY + local.y)
    let source = CGEventSource(stateID: .hidSystemState)
    let event: CGEvent?
    switch action {
    case .move:
        event = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                        mouseCursorPosition: screen, mouseButton: .left)
    case let .wheel(dx, dy):
        guard abs(dx) <= 64, abs(dy) <= 64 else {
            throw ProbeFailure.typed("wheel_delta_out_of_range", "wheel deltas must be within -64...64")
        }
        event = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 2,
                        wheel1: dy, wheel2: dx, wheel3: 0)
        event?.location = screen
    }
    guard let event else { throw ProbeFailure.typed("event_create_failed", "CoreGraphics refused the event") }
    try stamp(event, sky: sky, pid: pid, window: window, local: local)
    ledger.record()
    post(pid, Unmanaged.passUnretained(event).toOpaque())
}

private func publicBaseline(pid: pid_t, window: UInt32) throws {
    let bounds = try exactWindow(id: window, owner: pid)
    let screen = CGPoint(x: bounds.midX, y: bounds.midY)
    guard let event = CGEvent(mouseEventSource: CGEventSource(stateID: .hidSystemState),
                              mouseType: .mouseMoved, mouseCursorPosition: screen, mouseButton: .left) else {
        throw ProbeFailure.typed("event_create_failed", "CoreGraphics refused the baseline event")
    }
    event.postToPid(pid)
}

private func fixtureCountersEqual(_ lhs: FixtureState, _ rhs: FixtureState) -> Bool {
    lhs.keyWindow == rhs.keyWindow && lhs.windows.count == rhs.windows.count && lhs.windows.allSatisfy { label, before in
        guard let after = rhs.windows[label] else { return false }
        return before.moveSequence == after.moveSequence && before.wheelSequence == after.wheelSequence
    }
}

private func expectOneAdvance(
    before: FixtureState, after: FixtureState, label: String, kind: String,
    expectedDx: Int32 = 0, expectedDy: Int32 = 0
) throws -> WindowState {
    guard let beforeTarget = before.windows[label], let afterTarget = after.windows[label] else {
        throw ProbeFailure.typed("fixture_window_missing", "addressed fixture window is absent")
    }
    for other in before.windows.keys where other != label {
        guard let lhs = before.windows[other], let rhs = after.windows[other],
              lhs.moveSequence == rhs.moveSequence, lhs.wheelSequence == rhs.wheelSequence else {
            throw ProbeFailure.typed("misdelivery", "an unaddressed fixture window changed")
        }
    }
    if kind == "move" {
        guard afterTarget.moveSequence == beforeTarget.moveSequence + 1 else {
            throw ProbeFailure.typed("delivery_unverified", "move sequence did not advance exactly once")
        }
    } else {
        guard afterTarget.wheelSequence == beforeTarget.wheelSequence + 1 else {
            throw ProbeFailure.typed("delivery_unverified", "wheel sequence did not advance exactly once")
        }
        guard let actualX = afterTarget.lastDeltaX, let actualY = afterTarget.lastDeltaY,
              actualX == Double(expectedDx), actualY == Double(expectedDy) else {
            throw ProbeFailure.typed("wheel_delta_mismatch", "fixture observed different bounded wheel deltas")
        }
    }
    guard let x = afterTarget.lastX, let y = afterTarget.lastY,
          x >= 0, y >= 0, x <= 260, y <= 180, afterTarget.lastTimestamp != nil else {
        throw ProbeFailure.typed("native_event_incomplete", "fixture did not observe local coordinates and native timestamp")
    }
    return afterTarget
}

private func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name), index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}

private func writeJSON(_ value: Any) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

#if arch(arm64)
private let currentArchitecture = "arm64"
#elseif arch(x86_64)
private let currentArchitecture = "x86_64"
#else
private let currentArchitecture = "unknown"
#endif

private let measuredHostBuild = "25F80"

private func requireMeasuredHost() throws {
    let os = ProcessInfo.processInfo.operatingSystemVersionString
    guard currentArchitecture == "arm64", os.contains("(Build \(measuredHostBuild))") else {
        throw ProbeFailure.typed(
            "provider_unavailable",
            "private SkyLight ABI is not qualified for this OS build and architecture"
        )
    }
}

guard let targetPath = argument("--target-state"), let guardPath = argument("--guard-state"),
      let commandPath = argument("--target-command") else {
    fputs("usage: Probe --target-state PATH --guard-state PATH --target-command PATH [--mode court|repeat]\n", stderr)
    exit(2)
}

do {
    let sourceSHA = argument("--source-sha") ?? "unknown"
    let probeDigest = argument("--probe-digest") ?? "unknown"
    try requireMeasuredHost()
    let mode = argument("--mode") ?? "court"
    guard ["court", "repeat"].contains(mode) else {
        throw ProbeFailure.typed("mode_invalid", "mode must be court or repeat")
    }
    let target = try load(targetPath)
    let guardState = try load(guardPath)
    guard target.role == "target", guardState.role == "guard",
          let a = target.windows["A"], let b = target.windows["B"],
          guardState.windows["guard"] != nil else {
        throw ProbeFailure.typed("fixture_invalid", "owned target and guard fixture states are incomplete")
    }
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == guardState.pid else {
        throw ProbeFailure.typed("fixture_guard_not_frontmost", "owned guard is not the foreground application")
    }

    let sky = SkyLight()
    let missingSky = SkyLight(forceMissing: true)
    let ledger = AttemptLedger()
    let c1 = sky.available && sky.resolved.values.allSatisfy { $0 }
    guard c1 else { throw ProbeFailure.typed("provider_unavailable", "required current-host SkyLight symbols are missing") }
    do { try inject(sky: missingSky, ledger: ledger, pid: target.pid, window: a.id, action: .move) }
    catch ProbeFailure.typed(let code, _) where code == "provider_unavailable" {}
    guard ledger.count == 0 else { throw ProbeFailure.typed("negative_injected", "missing-symbol path reached injection") }

    if mode == "repeat" {
        let initialHost = hostState()
        for index in 0..<1000 {
            let label = index.isMultiple(of: 2) ? "A" : "B"
            let window = label == "A" ? a.id : b.id
            let kind = index.isMultiple(of: 4) || index % 4 == 1 ? "move" : "wheel"
            let action: Action = kind == "move"
                ? .move
                : .wheel(dx: index.isMultiple(of: 2) ? 0 : -1, dy: index.isMultiple(of: 2) ? 1 : 0)
            let beforeFixture = try load(targetPath)
            let beforeGuard = try load(guardPath)
            let beforeHost = hostState()
            try inject(sky: sky, ledger: ledger, pid: target.pid, window: window, action: action)
            let afterFixture = try waitState(targetPath, deadline: Date().addingTimeInterval(1.0)) { state in
                guard let old = beforeFixture.windows[label], let new = state.windows[label] else { return false }
                return kind == "move" ? new.moveSequence > old.moveSequence : new.wheelSequence > old.wheelSequence
            }
            let expectedDx: Int32 = kind == "wheel" && !index.isMultiple(of: 2) ? -1 : 0
            let expectedDy: Int32 = kind == "wheel" && index.isMultiple(of: 2) ? 1 : 0
            _ = try expectOneAdvance(
                before: beforeFixture, after: afterFixture, label: label, kind: kind,
                expectedDx: expectedDx, expectedDy: expectedDy
            )
            let afterGuard = try load(guardPath)
            guard fixtureCountersEqual(beforeGuard, afterGuard) else {
                throw ProbeFailure.typed("misdelivery", "the owned foreground guard received an event")
            }
            guard hostState() == beforeHost, beforeHost == initialHost else {
                throw ProbeFailure.typed("host_state_drift", "C8 changed pointer, foreground application or focused window")
            }
        }
        writeJSON([
            "ok": true, "schema": 1, "mode": "repeat", "C8": true,
            "actions": 1000, "injection_attempts": ledger.count,
            "source_sha": sourceSHA, "probe_digest": probeDigest,
        ])
        exit(0)
    }

    let baselineBefore = try load(targetPath)
    try publicBaseline(pid: target.pid, window: a.id)
    usleep(120_000)
    let baselineAfter = try load(targetPath)
    let baselineNoWindow = baselineAfter.windows["A"]?.moveSequence == baselineBefore.windows["A"]?.moveSequence

    var actionEvidence: [[String: Any]] = []
    for (label, window, action, kind, expectedDx, expectedDy) in [
        ("A", a.id, Action.move, "move", Int32(0), Int32(0)),
        ("B", b.id, Action.move, "move", Int32(0), Int32(0)),
        ("A", a.id, Action.wheel(dx: 0, dy: 3), "wheel", Int32(0), Int32(3)),
        ("B", b.id, Action.wheel(dx: -2, dy: 0), "wheel", Int32(-2), Int32(0)),
    ] {
        let beforeFixture = try load(targetPath)
        let beforeGuard = try load(guardPath)
        let beforeHost = hostState()
        try inject(sky: sky, ledger: ledger, pid: target.pid, window: window, action: action)
        let afterFixture = try waitState(targetPath, deadline: Date().addingTimeInterval(1.0)) { state in
            guard let old = beforeFixture.windows[label], let new = state.windows[label] else { return false }
            return kind == "move" ? new.moveSequence > old.moveSequence : new.wheelSequence > old.wheelSequence
        }
        let afterHost = hostState()
        let observed = try expectOneAdvance(
            before: beforeFixture, after: afterFixture, label: label, kind: kind,
            expectedDx: expectedDx, expectedDy: expectedDy
        )
        let afterGuard = try load(guardPath)
        guard fixtureCountersEqual(beforeGuard, afterGuard) else {
            throw ProbeFailure.typed("misdelivery", "the owned foreground guard received an event")
        }
        guard beforeHost == afterHost else {
            throw ProbeFailure.typed("host_state_drift", "pointer, foreground application or focused window changed")
        }
        actionEvidence.append([
            "window": label, "window_id": window, "kind": kind,
            "move_sequence": observed.moveSequence, "wheel_sequence": observed.wheelSequence,
            "local_x": observed.lastX!, "local_y": observed.lastY!,
            "delta_x": observed.lastDeltaX ?? 0, "delta_y": observed.lastDeltaY ?? 0,
            "native_timestamp": observed.lastTimestamp!,
            "guard_unchanged": true, "host_state_unchanged": true,
        ])
    }

    let beforeNegative = ledger.count
    for operation in [
        { try inject(sky: sky, ledger: ledger, pid: target.pid + 1, window: a.id, action: .move) },
        { try inject(sky: sky, ledger: ledger, pid: target.pid, window: UInt32.max, action: .move) },
        { try inject(sky: sky, ledger: ledger, pid: target.pid, window: 0, action: .move) },
    ] {
        do { try operation(); throw ProbeFailure.typed("negative_accepted", "invalid target passed validation") }
        catch ProbeFailure.typed(let code, _) where ["window_owner_changed", "stale_window"].contains(code) {}
    }
    do { try rejectAmbiguousOwnerTarget(owner: target.pid) }
    catch ProbeFailure.typed(let code, _) where code == "window_ambiguous" {}
    try Data("close-B\n".utf8).write(to: URL(fileURLWithPath: commandPath), options: .atomic)
    _ = try waitState(targetPath, deadline: Date().addingTimeInterval(1.0)) { $0.windows["B"] == nil }
    let closedDeadline = Date().addingTimeInterval(3.0)
    while Date() < closedDeadline, windowIsOnscreen(b.id) { usleep(10_000) }
    guard !windowIsOnscreen(b.id) else {
        throw ProbeFailure.typed("closed_window_still_onscreen", "closed target remained on-screen beyond the bounded deadline")
    }
    do { try inject(sky: sky, ledger: ledger, pid: target.pid, window: b.id, action: .move) }
    catch ProbeFailure.typed(let code, _) where ["stale_window", "window_not_onscreen"].contains(code) {}
    guard ledger.count == beforeNegative else {
        throw ProbeFailure.typed("negative_injected", "a rejected target reached injection")
    }

    writeJSON([
        "ok": true,
        "schema": 1,
        "host": ["os": ProcessInfo.processInfo.operatingSystemVersionString, "arch": currentArchitecture],
        "source_sha": sourceSHA,
        "probe_digest": probeDigest,
        "symbols": sky.resolved,
        "baseline_public_pid_no_window": baselineNoWindow,
        "criteria": ["C1": c1, "C2": true, "C3": true, "C4": true, "C5": true, "C6": true, "C7": true, "C8": NSNull()],
        "actions": actionEvidence,
        "injection_attempts": ledger.count,
    ])
} catch {
    writeJSON(["ok": false, "error": String(describing: error)])
    exit(1)
}
