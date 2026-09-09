import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation

// Research-only held-button injector for the precommitted section 10 court.
// It is intentionally not linked into an AgenTerm product binary.

private let measuredHostBuild = "25F80"
private let dragSteps = 20
private let cadenceMicroseconds: useconds_t = 6_000

private enum InjectorFailure: Error, CustomStringConvertible {
    case typed(String, String)

    var description: String {
        switch self {
        case let .typed(code, message): return "\(code): \(message)"
        }
    }
}

private enum Route: String {
    case `private`
    case publicLoc = "public-loc"
    case publicOff = "public-off"
}

private enum Mode: String {
    case gesture
    case releaseOnly = "release-only"
}

private struct GuardState: Decodable {
    let role: String
    let pid: Int32
}

private struct HostSample: Equatable {
    let stage: String
    let tNs: UInt64
    let pointerX: Double
    let pointerY: Double
    let frontmostPid: Int32
    let frontmostWindow: UInt32?
    let targetAXMainWindow: UInt32?
    let targetAXFocusedWindow: UInt32?

    func sameHostState(as baseline: HostSample) -> Bool {
        pointerX == baseline.pointerX
            && pointerY == baseline.pointerY
            && frontmostPid == baseline.frontmostPid
            && frontmostWindow == baseline.frontmostWindow
            && targetAXMainWindow == baseline.targetAXMainWindow
            && targetAXFocusedWindow == baseline.targetAXFocusedWindow
    }

    var json: [String: Any] {
        [
            "stage": stage,
            "t_ns": tNs,
            "pointer": ["x": pointerX, "y": pointerY],
            "frontmost_pid": frontmostPid,
            "frontmost_window": frontmostWindow.map { Int($0) as Any } ?? NSNull(),
            "target_ax_main_window": targetAXMainWindow.map { Int($0) as Any } ?? NSNull(),
            "target_ax_focused_window": targetAXFocusedWindow.map { Int($0) as Any } ?? NSNull(),
        ]
    }
}

private final class SkyLight {
    typealias Post = @convention(c) (pid_t, UnsafeMutableRawPointer?) -> Void
    typealias SetInteger = @convention(c) (UnsafeMutableRawPointer?, UInt32, Int64) -> Void
    typealias SetLocation = @convention(c) (UnsafeMutableRawPointer?, Double, Double) -> Void

    private let handle: UnsafeMutableRawPointer?
    let post: Post?
    let setInteger: SetInteger?
    let setLocation: SetLocation?

    init() {
        let loaded = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_NOW | RTLD_LOCAL
        )
        handle = loaded
        func resolve<T>(_ name: String, _: T.Type) -> T? {
            guard let loaded, let symbol = dlsym(loaded, name) else { return nil }
            return unsafeBitCast(symbol, to: T.self)
        }
        post = resolve("SLEventPostToPid", Post.self)
        setInteger = resolve("SLEventSetIntegerValueField", SetInteger.self)
        setLocation = resolve("CGEventSetWindowLocation", SetLocation.self)
    }

    deinit { if let handle { dlclose(handle) } }
    var available: Bool { post != nil && setInteger != nil && setLocation != nil }
}

private final class AXWindowReader {
    typealias GetWindow = @convention(c) (AXUIElement, UnsafeMutablePointer<CGWindowID>) -> AXError

    private let handle: UnsafeMutableRawPointer?
    private let getWindow: GetWindow?

    init() {
        let loaded = dlopen(
            "/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices",
            RTLD_NOW | RTLD_LOCAL
        )
        handle = loaded
        if let loaded, let symbol = dlsym(loaded, "_AXUIElementGetWindow") {
            getWindow = unsafeBitCast(symbol, to: GetWindow.self)
        } else {
            getWindow = nil
        }
    }

    deinit { if let handle { dlclose(handle) } }

    private func windowIdentity(pid: pid_t, attribute: CFString) throws -> UInt32? {
        guard let getWindow else {
            throw InjectorFailure.typed(
                "host_state_unavailable",
                "the AX focused-window identity symbol is unavailable"
            )
        }
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(
            app,
            attribute,
            &value
        )
        if status == .noValue || status == .attributeUnsupported { return nil }
        guard status == .success, let value else {
            throw InjectorFailure.typed(
                "host_state_unavailable",
                "the target application's AX focused window could not be sampled (\(status.rawValue))"
            )
        }
        let windowElement = unsafeBitCast(value, to: AXUIElement.self)
        var id: CGWindowID = 0
        let idStatus = getWindow(windowElement, &id)
        guard idStatus == .success else {
            throw InjectorFailure.typed(
                "host_state_unavailable",
                "the target AX focused window has no stable CGWindowID (\(idStatus.rawValue))"
            )
        }
        return id == 0 ? nil : id
    }

    func mainWindow(pid: pid_t) throws -> UInt32? {
        try windowIdentity(pid: pid, attribute: kAXMainWindowAttribute as CFString)
    }

    func focusedWindow(pid: pid_t) throws -> UInt32? {
        try windowIdentity(pid: pid, attribute: kAXFocusedWindowAttribute as CFString)
    }
}

private func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name),
          index + 1 < CommandLine.arguments.count else { return nil }
    return CommandLine.arguments[index + 1]
}

private func requiredArgument(_ name: String) throws -> String {
    guard let value = argument(name), !value.isEmpty else {
        throw InjectorFailure.typed("argument_missing", "missing \(name)")
    }
    return value
}

private func parseDouble(_ name: String) throws -> Double {
    let raw = try requiredArgument(name)
    guard let value = Double(raw), value.isFinite else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be finite")
    }
    return value
}

private func sameRect(_ left: CGRect, _ right: CGRect) -> Bool {
    left.origin.x == right.origin.x && left.origin.y == right.origin.y
        && left.width == right.width && left.height == right.height
}

private func parseInt32(_ name: String) throws -> Int32 {
    let raw = try requiredArgument(name)
    guard let value = Int32(raw), value > 1 else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be an integer greater than 1")
    }
    return value
}

private func parseUInt32(_ name: String) throws -> UInt32 {
    let raw = try requiredArgument(name)
    guard let value = UInt32(raw), value != 0 else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be a positive u32")
    }
    return value
}

private func parseMode() throws -> Mode {
    let raw = try requiredArgument("--mode")
    guard let mode = Mode(rawValue: raw) else {
        throw InjectorFailure.typed(
            "argument_invalid",
            "--mode must be gesture or release-only"
        )
    }
    return mode
}

private func parseRoute() throws -> Route {
    let raw = try requiredArgument("--arm")
    guard let route = Route(rawValue: raw) else {
        throw InjectorFailure.typed(
            "argument_invalid",
            "--arm must be private, public-loc or public-off"
        )
    }
    return route
}

private func requireMeasuredHost() throws {
    #if arch(arm64)
    let architecture = "arm64"
    #else
    let architecture = "unsupported"
    #endif
    var size = 0
    guard sysctlbyname("kern.osversion", nil, &size, nil, 0) == 0, size > 1 else {
        throw InjectorFailure.typed("provider_unavailable", "the host build cannot be read")
    }
    var bytes = [CChar](repeating: 0, count: size)
    guard sysctlbyname("kern.osversion", &bytes, &size, nil, 0) == 0 else {
        throw InjectorFailure.typed("provider_unavailable", "the host build cannot be read")
    }
    let hostBuild = String(cString: bytes)
    guard architecture == "arm64", hostBuild == measuredHostBuild else {
        throw InjectorFailure.typed(
            "provider_unavailable",
            "the private drag experiment is not qualified for this OS build and architecture"
        )
    }
}

private func windowRow(_ id: UInt32) -> [String: Any]? {
    let rows = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID)
        as? [[String: Any]] ?? []
    return rows.first {
        ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value == id
    }
}

private func exactWindow(id: UInt32, owner: pid_t) throws -> CGRect {
    guard let row = windowRow(id) else {
        throw InjectorFailure.typed("stale_window", "the exact CGWindowID is absent")
    }
    guard (row[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == owner else {
        throw InjectorFailure.typed(
            "window_owner_changed",
            "the exact CGWindowID owner does not match the frozen pid"
        )
    }
    guard (row[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
          (row[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true,
          let raw = row[kCGWindowBounds as String] as? [String: CGFloat] else {
        throw InjectorFailure.typed(
            "window_not_onscreen",
            "the exact target is not an on-screen layer-zero window"
        )
    }
    let bounds = CGRect(
        x: raw["X"] ?? .nan,
        y: raw["Y"] ?? .nan,
        width: raw["Width"] ?? .nan,
        height: raw["Height"] ?? .nan
    )
    guard bounds.origin.x.isFinite, bounds.origin.y.isFinite,
          bounds.width.isFinite, bounds.height.isFinite,
          bounds.width > 0, bounds.height > 0 else {
        throw InjectorFailure.typed(
            "window_geometry_invalid",
            "the exact target has invalid bounds"
        )
    }
    return bounds
}

private func frontmostWindow(pid: pid_t) -> UInt32? {
    let rows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] ?? []
    return rows.first {
        ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid
            && ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0
    }.flatMap { ($0[kCGWindowNumber as String] as? NSNumber)?.uint32Value }
}

private func sampleHost(
    stage: String,
    startedNs: UInt64,
    targetPid: pid_t,
    ax: AXWindowReader
) throws -> HostSample {
    let pointer = NSEvent.mouseLocation
    let frontmostPid = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    return HostSample(
        stage: stage,
        tNs: DispatchTime.now().uptimeNanoseconds - startedNs,
        pointerX: pointer.x,
        pointerY: pointer.y,
        frontmostPid: frontmostPid,
        frontmostWindow: frontmostWindow(pid: frontmostPid),
        targetAXMainWindow: try ax.mainWindow(pid: targetPid),
        targetAXFocusedWindow: try ax.focusedWindow(pid: targetPid)
    )
}

private func stamp(
    _ event: CGEvent,
    sky: SkyLight,
    pid: pid_t,
    window: UInt32,
    local: CGPoint,
    phase: Int64,
    group: Int64
) throws {
    guard let setInteger = sky.setInteger, let setLocation = sky.setLocation else {
        throw InjectorFailure.typed("provider_unavailable", "required SkyLight symbols are missing")
    }
    let raw = Unmanaged.passUnretained(event).toOpaque()
    for (field, value): (UInt32, Int64) in [
        (0, phase), (1, 1), (3, 0), (7, 3), (40, Int64(pid)),
        (51, Int64(window)), (58, group), (91, Int64(window)), (92, Int64(window)),
    ] {
        setInteger(raw, field, value)
    }
    setLocation(raw, local.x, local.y)
}

private struct PreparedEvent {
    let label: String
    let event: CGEvent
    let screen: CGPoint
}

private func preparedEvents(
    mode: Mode,
    route: Route,
    sky: SkyLight,
    pid: pid_t,
    window: UInt32,
    bounds: CGRect,
    from: CGPoint,
    to: CGPoint,
    offDelta: CGPoint
) throws -> [PreparedEvent] {
    let translated: (CGPoint) -> CGPoint = { point in
        route == .publicOff
            ? CGPoint(x: point.x + offDelta.x, y: point.y + offDelta.y)
            : point
    }
    let source = CGEventSource(stateID: .hidSystemState)
    let group = Int64(DispatchTime.now().uptimeNanoseconds % 1_000_000_000)
    var specifications: [(String, CGEventType, CGPoint, Int64)] = []
    if mode == .gesture {
        specifications.append(("down", .leftMouseDown, translated(from), 3))
        for index in 1...dragSteps {
            let progress = Double(index) / Double(dragSteps)
            let point = CGPoint(
                x: from.x + (to.x - from.x) * progress,
                y: from.y + (to.y - from.y) * progress
            )
            specifications.append(("move-\(index)", .leftMouseDragged, translated(point), 3))
        }
    }
    specifications.append(("up", .leftMouseUp, translated(to), 3))

    var events: [PreparedEvent] = []
    events.reserveCapacity(specifications.count)
    for (label, type, screen, phase) in specifications {
        guard let event = CGEvent(
            mouseEventSource: source,
            mouseType: type,
            mouseCursorPosition: screen,
            mouseButton: .left
        ) else {
            throw InjectorFailure.typed(
                "event_create_failed",
                "CoreGraphics refused a drag event before the button-down"
            )
        }
        if route == .private {
            let local = CGPoint(x: screen.x - bounds.minX, y: screen.y - bounds.minY)
            try stamp(
                event,
                sky: sky,
                pid: pid,
                window: window,
                local: local,
                phase: phase,
                group: group
            )
        }
        events.append(PreparedEvent(label: label, event: event, screen: screen))
    }
    return events
}

private func post(_ event: CGEvent, route: Route, pid: pid_t, sky: SkyLight) throws {
    switch route {
    case .private:
        guard let post = sky.post else {
            throw InjectorFailure.typed("provider_unavailable", "the SkyLight post symbol is missing")
        }
        post(pid, Unmanaged.passUnretained(event).toOpaque())
    case .publicLoc, .publicOff:
        event.postToPid(pid)
    }
}

private func writeJSON(_ value: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

private func run() throws -> [String: Any] {
    try requireMeasuredHost()
    let mode = try parseMode()
    let route = try parseRoute()
    let pid = try parseInt32("--pid")
    let window = try parseUInt32("--window")
    let frozenBounds = CGRect(
        x: try parseDouble("--window-x"),
        y: try parseDouble("--window-y"),
        width: try parseDouble("--window-width"),
        height: try parseDouble("--window-height")
    )
    guard frozenBounds.width > 0, frozenBounds.height > 0 else {
        throw InjectorFailure.typed("argument_invalid", "the frozen window size must be positive")
    }
    let from = CGPoint(x: try parseDouble("--start-x"), y: try parseDouble("--start-y"))
    let to = CGPoint(x: try parseDouble("--end-x"), y: try parseDouble("--end-y"))
    let offDelta = CGPoint(x: try parseDouble("--off-dx"), y: try parseDouble("--off-dy"))
    let steps = argument("--steps").flatMap(Int.init)
    let cadenceUs = argument("--cadence-us").flatMap(UInt32.init)
    guard steps == dragSteps else {
        throw InjectorFailure.typed("argument_invalid", "--steps must be exactly 20")
    }
    guard cadenceUs == cadenceMicroseconds else {
        throw InjectorFailure.typed("argument_invalid", "--cadence-us must be exactly 6000")
    }
    let timeoutMs = argument("--timeout-ms").flatMap(Int.init) ?? 5_000
    guard (100...30_000).contains(timeoutMs) else {
        throw InjectorFailure.typed("argument_invalid", "--timeout-ms must be 100...30000")
    }

    let guardPath = try requiredArgument("--guard-state")
    let guardState = try JSONDecoder().decode(
        GuardState.self,
        from: Data(contentsOf: URL(fileURLWithPath: guardPath))
    )
    guard guardState.role == "guard",
          NSWorkspace.shared.frontmostApplication?.processIdentifier == guardState.pid else {
        throw InjectorFailure.typed(
            "fixture_guard_not_frontmost",
            "the owned guard is not the foreground application"
        )
    }

    let currentBounds = try exactWindow(id: window, owner: pid)
    if mode == .gesture && !sameRect(currentBounds, frozenBounds) {
        throw InjectorFailure.typed(
            "window_geometry_changed",
            "the exact target geometry changed before the button-down"
        )
    }
    guard mode == .releaseOnly || frozenBounds.contains(from) else {
        throw InjectorFailure.typed("path_invalid", "the drag start is outside the exact target")
    }
    let offFrom = CGPoint(x: from.x + offDelta.x, y: from.y + offDelta.y)
    if mode == .gesture && route == .publicOff && frozenBounds.contains(offFrom) {
        throw InjectorFailure.typed("control_path_invalid", "the public-off path must start outside the target")
    }

    let sky = SkyLight()
    if route == .private && !sky.available {
        throw InjectorFailure.typed("provider_unavailable", "required SkyLight symbols are missing")
    }
    let ax = AXWindowReader()
    let events = try preparedEvents(
        mode: mode,
        route: route,
        sky: sky,
        pid: pid,
        window: window,
        bounds: frozenBounds,
        from: from,
        to: to,
        offDelta: offDelta
    )

    let startedNs = DispatchTime.now().uptimeNanoseconds
    let deadlineNs = startedNs + UInt64(timeoutMs) * 1_000_000
    let baseline = try sampleHost(stage: "before", startedNs: startedNs, targetPid: pid, ax: ax)
    guard baseline.frontmostPid == guardState.pid else {
        throw InjectorFailure.typed("fixture_guard_changed", "the foreground guard changed before injection")
    }

    var samples = [baseline]
    var downAttempts = 0
    var moveAttempts = 0
    var upAttempts = 0
    var sameIdentity = true
    var maxPostToSampleNs: UInt64 = 0
    var failureCode: String?
    var failureMessage: String?
    var downPostUncertain = false
    var upPostUncertain = false

    func recordFailure(_ code: String, _ message: String) {
        if failureCode == nil {
            failureCode = code
            failureMessage = message
        }
    }

    func sampleAfter(_ label: String, postedNs: UInt64) {
        do {
            let sample = try sampleHost(stage: label, startedNs: startedNs, targetPid: pid, ax: ax)
            samples.append(sample)
            maxPostToSampleNs = max(
                maxPostToSampleNs,
                DispatchTime.now().uptimeNanoseconds - postedNs
            )
            if DispatchTime.now().uptimeNanoseconds - postedNs > 50_000_000 {
                recordFailure(
                    "host_sample_deadline_exceeded",
                    "a post-to-host-sample interval exceeded 50 milliseconds"
                )
            }
            if !sample.sameHostState(as: baseline) {
                recordFailure("host_state_changed", "pointer, foreground or AX focused-window identity changed")
            }
        } catch let InjectorFailure.typed(code, message) {
            recordFailure(code, message)
        } catch {
            recordFailure("host_state_unavailable", "a host-state sample failed")
        }
    }

    if mode == .releaseOnly {
        upAttempts = 1
        let postedNs = DispatchTime.now().uptimeNanoseconds
        do {
            try post(events[0].event, route: route, pid: pid, sky: sky)
        } catch let InjectorFailure.typed(code, message) {
            upPostUncertain = true
            recordFailure(code, message)
        } catch {
            upPostUncertain = true
            recordFailure("release_failed", "the release-only native post failed")
        }
        sampleAfter("after-up", postedNs: postedNs)
    } else {
        // Every event is retained and fully stamped before this first post.
        downAttempts = 1
        let downPostedNs = DispatchTime.now().uptimeNanoseconds
        do {
            try post(events[0].event, route: route, pid: pid, sky: sky)
        } catch let InjectorFailure.typed(code, message) {
            downPostUncertain = true
            recordFailure(code, message)
        } catch {
            downPostUncertain = true
            recordFailure("down_post_failed", "the button-down native post failed")
        }
        if failureCode == nil { usleep(cadenceMicroseconds) }
        sampleAfter("after-down", postedNs: downPostedNs)

        if failureCode == nil {
            for event in events.dropFirst().dropLast() {
                if DispatchTime.now().uptimeNanoseconds >= deadlineNs {
                    recordFailure("injector_timeout", "the bounded drag deadline expired")
                    break
                }
                do {
                    let observedBounds = try exactWindow(id: window, owner: pid)
                    guard sameRect(observedBounds, frozenBounds) else {
                        throw InjectorFailure.typed(
                            "window_geometry_changed",
                            "the exact target geometry changed during the gesture"
                        )
                    }
                    moveAttempts += 1
                    let postedNs = DispatchTime.now().uptimeNanoseconds
                    try post(event.event, route: route, pid: pid, sky: sky)
                    usleep(cadenceMicroseconds)
                    sampleAfter("after-\(event.label)", postedNs: postedNs)
                } catch let InjectorFailure.typed(code, message) {
                    sameIdentity = false
                    recordFailure(code, message)
                } catch {
                    recordFailure("injector_failed", "a dragged-move native post failed")
                }
                if failureCode != nil { break }
            }
        }

        // A down attempt owns exactly one same-route, same-pid, same-window up
        // attempt even if identity, timing or host-state checks failed above.
        let release = events[events.count - 1]
        upAttempts = 1
        let upPostedNs = DispatchTime.now().uptimeNanoseconds
        do {
            try post(release.event, route: route, pid: pid, sky: sky)
        } catch let InjectorFailure.typed(code, message) {
            upPostUncertain = true
            recordFailure(code, message)
        } catch {
            upPostUncertain = true
            recordFailure("release_failed", "the button-up native post failed")
        }
        sampleAfter("after-up", postedNs: upPostedNs)
        if let observedBounds = try? exactWindow(id: window, owner: pid),
           !sameRect(observedBounds, frozenBounds) {
            sameIdentity = false
            recordFailure("window_geometry_changed", "the exact target geometry changed during the gesture")
        } else if (try? exactWindow(id: window, owner: pid)) == nil {
            sameIdentity = false
            recordFailure("window_identity_changed", "the exact target identity changed during the gesture")
        }
    }

    if mode == .releaseOnly && (try? exactWindow(id: window, owner: pid)) == nil {
        sameIdentity = false
        recordFailure("window_identity_changed", "the exact target identity changed during release recovery")
    }

    let expectedSamples = mode == .gesture ? dragSteps + 3 : 2
    let hostSamplesComplete = samples.count == expectedSamples
    if !hostSamplesComplete {
        recordFailure("host_samples_incomplete", "an intermediate host-state sample is missing")
    }
    let ok = failureCode == nil
    let outcomeUnknown = mode == .releaseOnly || downPostUncertain
        || upPostUncertain || upAttempts != 1
    return [
        "ok": ok && hostSamplesComplete && sameIdentity,
        "schema": 1,
        "mode": mode.rawValue,
        "arm": route.rawValue,
        "pid": pid,
        "window": window,
        "frozen_window_bounds": [
            "x": frozenBounds.minX,
            "y": frozenBounds.minY,
            "width": frozenBounds.width,
            "height": frozenBounds.height,
        ],
        "steps": dragSteps,
        "cadence_us": Int(cadenceMicroseconds),
        "from": ["x": from.x, "y": from.y],
        "to": ["x": to.x, "y": to.y],
        "off_delta": ["x": offDelta.x, "y": offDelta.y],
        "down_attempts": downAttempts,
        "move_attempts": moveAttempts,
        "up_attempts": upAttempts,
        "up_attempted": upAttempts == 1,
        "up_post_uncertain": upPostUncertain,
        "up_proven": false,
        "up_proof": "parent-page-oracle-required",
        "same_identity": sameIdentity,
        "host_samples_complete": hostSamplesComplete,
        "host_unchanged": samples.allSatisfy { $0.sameHostState(as: baseline) },
        "max_post_to_sample_ms": Double(maxPostToSampleNs) / 1_000_000.0,
        "host_samples": samples.map(\.json),
        "outcome_unknown": outcomeUnknown,
        "outcome": ok
            ? (mode == .releaseOnly ? "release-posted-awaiting-oracle" : "posted-awaiting-oracle")
            : (downAttempts == 0 ? "failed-before-down" : "failed-after-down-release-attempted"),
        "error": failureCode.map {
            ["code": $0, "message": failureMessage ?? ""] as Any
        } ?? NSNull(),
    ]
}

do {
    let receipt = try run()
    writeJSON(receipt)
    if receipt["ok"] as? Bool != true { exit(1) }
} catch let InjectorFailure.typed(code, message) {
    writeJSON([
        "ok": false,
        "schema": 1,
        "down_attempts": 0,
        "move_attempts": 0,
        "up_attempts": 0,
        "up_attempted": false,
        "up_post_uncertain": false,
        "up_proven": false,
        "same_identity": false,
        "host_samples_complete": false,
        "host_unchanged": false,
        "max_post_to_sample_ms": 0,
        "outcome_unknown": false,
        "outcome": "failed-before-down",
        "error": ["code": code, "message": message],
    ])
    exit(1)
} catch {
    writeJSON([
        "ok": false,
        "schema": 1,
        "down_attempts": 0,
        "move_attempts": 0,
        "up_attempts": 0,
        "up_attempted": false,
        "up_post_uncertain": false,
        "up_proven": false,
        "same_identity": false,
        "host_samples_complete": false,
        "host_unchanged": false,
        "max_post_to_sample_ms": 0,
        "outcome_unknown": false,
        "outcome": "failed-before-down",
        "error": ["code": "injector_failed", "message": "the injector failed before the button-down"],
    ])
    exit(1)
}
