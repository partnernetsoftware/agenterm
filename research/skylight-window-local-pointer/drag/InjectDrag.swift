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
private let focusReadbackDelayMicroseconds: useconds_t = 40_000

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
    case leaseOnly = "lease-only"
}

private struct GuardState: Decodable {
    let role: String
    let pid: Int32
}

private struct PhaseMessage: Codable, Equatable {
    let schema: Int
    let sequence: Int
    let phase: String
}

private struct PhaseAck: Codable {
    let schema: Int
    let sequence: Int
    let phase: String
    let proceed: Bool
}

private struct PhaseReceipt {
    let sequence: Int
    let phase: String
    let published: Bool
    let publishedNs: UInt64
    var acknowledgedNs: UInt64?

    var json: [String: Any] {
        [
            "sequence": sequence,
            "phase": phase,
            "published": published,
            "published_ns": publishedNs,
            "acknowledged_ns": acknowledgedNs.map { $0 as Any } ?? NSNull(),
            "acknowledged": acknowledgedNs != nil,
        ]
    }

}

private struct FocusTuple: Codable, Equatable {
    let appFocusedWindow: UInt32?
    let peerMain: Bool?
    let peerFocused: Bool?
    let targetMain: Bool?
    let targetFocused: Bool?

    private enum CodingKeys: String, CodingKey {
        case appFocusedWindow = "app_focused_window"
        case peer
        case target
    }

    private enum WindowCodingKeys: String, CodingKey {
        case main
        case focused
    }

    init(
        appFocusedWindow: UInt32?,
        peerMain: Bool?,
        peerFocused: Bool?,
        targetMain: Bool?,
        targetFocused: Bool?
    ) {
        self.appFocusedWindow = appFocusedWindow
        self.peerMain = peerMain
        self.peerFocused = peerFocused
        self.targetMain = targetMain
        self.targetFocused = targetFocused
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        appFocusedWindow = try values.decodeIfPresent(UInt32.self, forKey: .appFocusedWindow)
        let peer = try values.nestedContainer(keyedBy: WindowCodingKeys.self, forKey: .peer)
        peerMain = try peer.decodeIfPresent(Bool.self, forKey: .main)
        peerFocused = try peer.decodeIfPresent(Bool.self, forKey: .focused)
        let target = try values.nestedContainer(keyedBy: WindowCodingKeys.self, forKey: .target)
        targetMain = try target.decodeIfPresent(Bool.self, forKey: .main)
        targetFocused = try target.decodeIfPresent(Bool.self, forKey: .focused)
    }

    func encode(to encoder: Encoder) throws {
        var values = encoder.container(keyedBy: CodingKeys.self)
        try values.encodeIfPresent(appFocusedWindow, forKey: .appFocusedWindow)
        var peer = values.nestedContainer(keyedBy: WindowCodingKeys.self, forKey: .peer)
        try peer.encodeIfPresent(peerMain, forKey: .main)
        try peer.encodeIfPresent(peerFocused, forKey: .focused)
        var target = values.nestedContainer(keyedBy: WindowCodingKeys.self, forKey: .target)
        try target.encodeIfPresent(targetMain, forKey: .main)
        try target.encodeIfPresent(targetFocused, forKey: .focused)
    }

    var json: [String: Any] {
        [
            "app_focused_window": appFocusedWindow.map { Int($0) as Any } ?? NSNull(),
            "peer": [
                "main": peerMain.map { $0 as Any } ?? NSNull(),
                "focused": peerFocused.map { $0 as Any } ?? NSNull(),
            ],
            "target": [
                "main": targetMain.map { $0 as Any } ?? NSNull(),
                "focused": targetFocused.map { $0 as Any } ?? NSNull(),
            ],
        ]
    }

    func unavailableFields(_ phase: String) -> [String] {
        var fields: [String] = []
        if appFocusedWindow == nil { fields.append("\(phase).application.focused_window") }
        if peerMain == nil { fields.append("\(phase).peer.main") }
        if peerFocused == nil { fields.append("\(phase).peer.focused") }
        if targetMain == nil { fields.append("\(phase).target.main") }
        if targetFocused == nil { fields.append("\(phase).target.focused") }
        return fields
    }
}

private struct WindowOrder: Equatable {
    let window: UInt32
    let owner: Int32
    let rank: Int?
    let layer: Int?

    var json: [String: Any] {
        [
            "window": Int(window),
            "owner": owner,
            "rank": rank.map { $0 as Any } ?? NSNull(),
            "layer": layer.map { $0 as Any } ?? NSNull(),
        ]
    }
}

private struct HostSample: Equatable {
    let stage: String
    let tNs: UInt64
    let pointerX: Double
    let pointerY: Double
    let frontmostPid: Int32
    let frontmostWindow: UInt32?
    let guardPid: Int32
    let guardWindow: UInt32
    let peerOrder: WindowOrder
    let targetOrder: WindowOrder
    let guardOrder: WindowOrder
    let zOrderDigest: String
    let focus: FocusTuple

    func sameG6a(as baseline: HostSample) -> Bool {
        pointerX == baseline.pointerX
            && pointerY == baseline.pointerY
            && frontmostPid == baseline.frontmostPid
            && frontmostWindow == baseline.frontmostWindow
            && guardPid == baseline.guardPid
            && guardWindow == baseline.guardWindow
            && peerOrder == baseline.peerOrder
            && targetOrder == baseline.targetOrder
            && guardOrder == baseline.guardOrder
            && zOrderDigest == baseline.zOrderDigest
    }

    var json: [String: Any] {
        [
            "stage": stage,
            "t_ns": tNs,
            "g6a": [
                "pointer": ["x": pointerX, "y": pointerY],
                "frontmost_pid": frontmostPid,
                "frontmost_window": frontmostWindow.map { Int($0) as Any } ?? NSNull(),
                "guard": ["pid": guardPid, "window": Int(guardWindow)],
                "z_order_digest": zOrderDigest,
                "z_order": [peerOrder.json, targetOrder.json, guardOrder.json],
            ],
            "focus": focus.json,
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

    private func windowIdentity(_ element: AXUIElement) throws -> UInt32? {
        guard let getWindow else {
            throw InjectorFailure.typed(
                "focus_tuple_unavailable",
                "the AX window identity symbol is unavailable"
            )
        }
        var id: CGWindowID = 0
        let status = getWindow(element, &id)
        guard status == .success else {
            throw InjectorFailure.typed(
                "focus_tuple_unavailable",
                "an AX window has no stable CGWindowID"
            )
        }
        return id == 0 ? nil : id
    }

    private func windowAttribute(pid: pid_t, attribute: CFString) throws -> UInt32? {
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(app, attribute, &value)
        if status == .noValue || status == .attributeUnsupported { return nil }
        guard status == .success, let value else {
            throw InjectorFailure.typed(
                "focus_tuple_unavailable",
                "the application AX window identity could not be sampled"
            )
        }
        let windowElement = unsafeBitCast(value, to: AXUIElement.self)
        return try windowIdentity(windowElement)
    }

    private func windowElement(pid: pid_t, id: UInt32) throws -> AXUIElement {
        let app = AXUIElementCreateApplication(pid)
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(
            app, kAXWindowsAttribute as CFString, &value
        )
        guard status == .success, let windows = value as? [AXUIElement] else {
            throw InjectorFailure.typed(
                "focus_tuple_unavailable",
                "the application AX window inventory could not be sampled"
            )
        }
        var matches: [AXUIElement] = []
        for window in windows where try windowIdentity(window) == id {
            matches.append(window)
        }
        guard matches.count == 1 else {
            throw InjectorFailure.typed(
                "focus_tuple_ambiguous",
                "the exact CGWindowID does not name one AX window"
            )
        }
        return matches[0]
    }

    private func boolAttribute(_ element: AXUIElement, _ attribute: CFString) throws -> Bool? {
        var value: CFTypeRef?
        let status = AXUIElementCopyAttributeValue(element, attribute, &value)
        if status == .noValue || status == .attributeUnsupported { return nil }
        guard status == .success, let number = value as? NSNumber else {
            throw InjectorFailure.typed(
                "focus_tuple_unavailable",
                "an AX focus tuple field could not be sampled"
            )
        }
        return number.boolValue
    }

    func tuple(pid: pid_t, peer: UInt32, target: UInt32) throws -> FocusTuple {
        let peerElement = try windowElement(pid: pid, id: peer)
        let targetElement = try windowElement(pid: pid, id: target)
        return FocusTuple(
            appFocusedWindow: try windowAttribute(
                pid: pid, attribute: kAXFocusedWindowAttribute as CFString
            ),
            peerMain: try boolAttribute(peerElement, kAXMainAttribute as CFString),
            peerFocused: try boolAttribute(peerElement, kAXFocusedAttribute as CFString),
            targetMain: try boolAttribute(targetElement, kAXMainAttribute as CFString),
            targetFocused: try boolAttribute(targetElement, kAXFocusedAttribute as CFString)
        )
    }

    func setFocusedWindow(pid: pid_t, id: UInt32) throws {
        let app = AXUIElementCreateApplication(pid)
        let window = try windowElement(pid: pid, id: id)
        let status = AXUIElementSetAttributeValue(
            app, kAXFocusedWindowAttribute as CFString, window
        )
        guard status == .success else {
            throw InjectorFailure.typed(
                "focus_lease_set_failed",
                "the application focused window could not be changed"
            )
        }
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

private func focusTupleArgument(_ name: String) throws -> FocusTuple {
    let raw = try requiredArgument(name)
    guard let data = raw.data(using: .utf8) else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be UTF-8 JSON")
    }
    do {
        return try JSONDecoder().decode(FocusTuple.self, from: data)
    } catch {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be one exact focus tuple")
    }
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
            "--mode must be gesture, release-only or lease-only"
        )
    }
    return mode
}

private func phaseHandshake(
    statePath: String,
    ackPath: String,
    sequence: Int,
    phase: String,
    startedNs: UInt64,
    deadlineNs: UInt64
) throws -> PhaseReceipt {
    let message = PhaseMessage(schema: 1, sequence: sequence, phase: phase)
    let publishedNs = DispatchTime.now().uptimeNanoseconds - startedNs
    do {
        let data = try JSONEncoder().encode(message)
        try data.write(to: URL(fileURLWithPath: statePath), options: .atomic)
    } catch {
        throw InjectorFailure.typed(
            "phase_handshake_publish_failed",
            "the bounded phase state could not be published"
        )
    }
    while DispatchTime.now().uptimeNanoseconds < deadlineNs {
        if let data = try? Data(contentsOf: URL(fileURLWithPath: ackPath)),
           let ack = try? JSONDecoder().decode(PhaseAck.self, from: data),
           ack.schema == message.schema,
           ack.sequence == message.sequence,
           ack.phase == message.phase {
            guard ack.proceed else {
                throw InjectorFailure.typed(
                    "phase_handshake_rejected",
                    "the parent rejected the observed focus phase"
                )
            }
            return PhaseReceipt(
                sequence: sequence,
                phase: phase,
                published: true,
                publishedNs: publishedNs,
                acknowledgedNs: DispatchTime.now().uptimeNanoseconds - startedNs
            )
        }
        usleep(10_000)
    }
    throw InjectorFailure.typed(
        "phase_handshake_timeout",
        "the parent did not acknowledge the bounded focus phase before the deadline"
    )
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

private func fnvDigest(_ text: String) -> String {
    var hash: UInt64 = 14_695_981_039_346_656_037
    for byte in text.utf8 {
        hash ^= UInt64(byte)
        hash = hash &* 1_099_511_628_211
    }
    return String(format: "%016llx", hash)
}

private func orderSnapshot(
    peer: UInt32,
    target: UInt32,
    guardWindow: UInt32,
    targetPid: pid_t,
    guardPid: pid_t
) throws -> (WindowOrder, WindowOrder, WindowOrder, String) {
    let rows = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID)
        as? [[String: Any]] ?? []
    func order(_ id: UInt32, owner: pid_t) throws -> WindowOrder {
        var match: (Int, [String: Any])?
        for index in 0..<rows.count {
            let row = rows[index]
            if (row[kCGWindowNumber as String] as? NSNumber)?.uint32Value == id {
                guard match == nil else {
                    throw InjectorFailure.typed(
                        "z_order_ambiguous", "a frozen window occurs more than once in the window list"
                    )
                }
                match = (index, row)
            }
        }
        guard let match,
              (match.1[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == owner else {
            throw InjectorFailure.typed(
                "z_order_unavailable", "a frozen window is absent or has changed owner"
            )
        }
        return WindowOrder(
            window: id,
            owner: owner,
            rank: match.0,
            layer: (match.1[kCGWindowLayer as String] as? NSNumber)?.intValue
        )
    }
    let peerOrder = try order(peer, owner: targetPid)
    let targetOrder = try order(target, owner: targetPid)
    let guardOrder = try order(guardWindow, owner: guardPid)
    let material = [peerOrder, targetOrder, guardOrder].map {
        "\($0.window):\($0.owner):\($0.rank ?? -1):\($0.layer ?? -1)"
    }.joined(separator: "|")
    return (peerOrder, targetOrder, guardOrder, fnvDigest(material))
}

private func sampleHost(
    stage: String,
    startedNs: UInt64,
    targetPid: pid_t,
    peerWindow: UInt32,
    targetWindow: UInt32,
    guardPid: pid_t,
    guardWindow: UInt32,
    ax: AXWindowReader
) throws -> HostSample {
    let pointer = NSEvent.mouseLocation
    let frontmostPid = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    let order = try orderSnapshot(
        peer: peerWindow,
        target: targetWindow,
        guardWindow: guardWindow,
        targetPid: targetPid,
        guardPid: guardPid
    )
    return HostSample(
        stage: stage,
        tNs: DispatchTime.now().uptimeNanoseconds - startedNs,
        pointerX: pointer.x,
        pointerY: pointer.y,
        frontmostPid: frontmostPid,
        frontmostWindow: frontmostWindow(pid: frontmostPid),
        guardPid: guardPid,
        guardWindow: guardWindow,
        peerOrder: order.0,
        targetOrder: order.1,
        guardOrder: order.2,
        zOrderDigest: order.3,
        focus: try ax.tuple(pid: targetPid, peer: peerWindow, target: targetWindow)
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
    if mode != .leaseOnly {
        specifications.append(("up", .leftMouseUp, translated(to), 3))
    }

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
        if mode != .leaseOnly {
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
    let peerWindow = try parseUInt32("--peer-window")
    let guardWindow = try parseUInt32("--guard-window")
    guard window != peerWindow, window != guardWindow, peerWindow != guardWindow else {
        throw InjectorFailure.typed(
            "argument_invalid", "target, peer and guard windows must be distinct"
        )
    }
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
    let phaseStatePath = try requiredArgument("--phase-state")
    let phaseAckPath = try requiredArgument("--phase-ack")
    let expectedPriorFocus = mode == .leaseOnly
        ? nil : try focusTupleArgument("--expected-prior-focus")
    let expectedTargetFocus = mode == .leaseOnly
        ? nil : try focusTupleArgument("--expected-target-focus")
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
    _ = try exactWindow(id: peerWindow, owner: pid)
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
    if mode != .leaseOnly && !sky.available {
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
    let baseline = try sampleHost(
        stage: "before-acquire",
        startedNs: startedNs,
        targetPid: pid,
        peerWindow: peerWindow,
        targetWindow: window,
        guardPid: guardState.pid,
        guardWindow: guardWindow,
        ax: ax
    )
    guard baseline.frontmostPid == guardState.pid,
          baseline.frontmostWindow == guardWindow else {
        throw InjectorFailure.typed("fixture_guard_changed", "the foreground guard changed before injection")
    }

    let initialFocus = baseline.focus
    if mode != .releaseOnly && initialFocus.appFocusedWindow != peerWindow {
        throw InjectorFailure.typed(
            "focus_tuple_precondition_failed",
            "the application focus tuple does not start on the frozen peer window"
        )
    }
    if let expectedPriorFocus, mode == .gesture, initialFocus != expectedPriorFocus {
        throw InjectorFailure.typed(
            "focus_tuple_precondition_failed",
            "the application focus tuple differs from the dry-cycle prior tuple"
        )
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
    var acquireAttempted = false
    var acquireVerified = false
    var acquireReadback: FocusTuple?
    var acquireStartedNs: UInt64?
    var acquireVerifiedNs: UInt64?
    var restoreAttempted = false
    var restoreVerified = false
    var restoreReadback: FocusTuple?
    var restoreStartedNs: UInt64?
    var restoreVerifiedNs: UInt64?
    var priorFocus: FocusTuple? = expectedPriorFocus ?? (mode == .releaseOnly ? nil : initialFocus)
    var targetFocus: FocusTuple? = expectedTargetFocus ?? (mode == .releaseOnly ? initialFocus : nil)
    var phaseReceipts: [PhaseReceipt] = []

    func recordFailure(_ code: String, _ message: String) {
        if failureCode == nil {
            failureCode = code
            failureMessage = message
        }
    }

    func sample(_ label: String) -> HostSample? {
        do {
            let observed = try sampleHost(
                stage: label,
                startedNs: startedNs,
                targetPid: pid,
                peerWindow: peerWindow,
                targetWindow: window,
                guardPid: guardState.pid,
                guardWindow: guardWindow,
                ax: ax
            )
            samples.append(observed)
            if !observed.sameG6a(as: baseline) {
                recordFailure(
                    "g6a_host_state_changed",
                    "pointer, foreground, guard identity or frozen window order changed"
                )
            }
            return observed
        } catch let InjectorFailure.typed(code, message) {
            recordFailure(code, message)
        } catch {
            recordFailure("host_state_unavailable", "a bounded host-state sample failed")
        }
        return nil
    }

    func sampleAfter(_ label: String, postedNs: UInt64, expectedFocus: FocusTuple?) {
        if let observed = sample(label) {
            let elapsedNs = DispatchTime.now().uptimeNanoseconds - postedNs
            maxPostToSampleNs = max(maxPostToSampleNs, elapsedNs)
            if elapsedNs > 50_000_000 {
                recordFailure(
                    "host_sample_deadline_exceeded",
                    "a post-to-host-sample interval exceeded 50 milliseconds"
                )
            }
            if let expectedFocus, observed.focus != expectedFocus {
                recordFailure(
                    "focus_lease_drift",
                    "the application focus tuple left the acquired target phase"
                )
            }
        }
    }

    func handshake(_ sequence: Int, _ phase: String) {
        let attemptedNs = DispatchTime.now().uptimeNanoseconds - startedNs
        do {
            let receipt = try phaseHandshake(
                statePath: phaseStatePath,
                ackPath: phaseAckPath,
                sequence: sequence,
                phase: phase,
                startedNs: startedNs,
                deadlineNs: deadlineNs
            )
            phaseReceipts.append(receipt)
        } catch let InjectorFailure.typed(code, message) {
            phaseReceipts.append(PhaseReceipt(
                sequence: sequence,
                phase: phase,
                published: code != "phase_handshake_publish_failed",
                publishedNs: attemptedNs,
                acknowledgedNs: nil
            ))
            recordFailure(code, message)
        } catch {
            phaseReceipts.append(PhaseReceipt(
                sequence: sequence,
                phase: phase,
                published: false,
                publishedNs: attemptedNs,
                acknowledgedNs: nil
            ))
            recordFailure("phase_handshake_failed", "the bounded phase handshake failed")
        }
    }

    func acquireTarget() {
        acquireAttempted = true
        acquireStartedNs = DispatchTime.now().uptimeNanoseconds - startedNs
        do {
            try ax.setFocusedWindow(pid: pid, id: window)
            usleep(focusReadbackDelayMicroseconds)
            if let acquired = sample("after-acquire") {
                acquireReadback = acquired.focus
                if targetFocus == nil { targetFocus = acquired.focus }
                acquireVerified = acquired.focus.appFocusedWindow == window
                    && acquired.focus == targetFocus
                    && acquired.sameG6a(as: baseline)
                if !acquireVerified {
                    recordFailure(
                        "focus_lease_acquire_unverified",
                        "the target focus tuple or frozen host state was not verified"
                    )
                } else {
                    acquireVerifiedNs = DispatchTime.now().uptimeNanoseconds - startedNs
                }
            }
        } catch let InjectorFailure.typed(code, message) {
            recordFailure(code, message)
        } catch {
            recordFailure("focus_lease_acquire_failed", "the target focus lease could not be acquired")
        }
        if acquireVerified { handshake(1, "acquired") }
    }

    func restorePeer() {
        restoreAttempted = true
        restoreStartedNs = DispatchTime.now().uptimeNanoseconds - startedNs
        do {
            try ax.setFocusedWindow(pid: pid, id: peerWindow)
            usleep(focusReadbackDelayMicroseconds)
            if let restored = sample("after-restore") {
                restoreReadback = restored.focus
                if priorFocus == nil { priorFocus = restored.focus }
                restoreVerified = restored.focus.appFocusedWindow == peerWindow
                    && restored.focus == priorFocus
                    && restored.sameG6a(as: baseline)
                if !restoreVerified {
                    recordFailure(
                        "focus_lease_restore_unverified",
                        "the prior focus tuple or frozen host state was not restored"
                    )
                } else {
                    restoreVerifiedNs = DispatchTime.now().uptimeNanoseconds - startedNs
                }
            }
        } catch let InjectorFailure.typed(code, message) {
            recordFailure(code, message)
        } catch {
            recordFailure("focus_lease_restore_failed", "the prior focus tuple could not be restored")
        }
        if restoreVerified { handshake(2, "restored") }
    }

    if mode == .leaseOnly {
        acquireTarget()
        restorePeer()
    } else if mode == .releaseOnly {
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
        sampleAfter("after-up", postedNs: postedNs, expectedFocus: nil)
        restorePeer()
    } else {
        acquireTarget()
        if failureCode == nil, acquireVerified, let targetFocus,
           let beforeDown = sample("before-down"), beforeDown.focus != targetFocus {
            recordFailure(
                "focus_lease_drift",
                "the application focus tuple changed before the button-down"
            )
        }
        // Every event is retained and fully stamped before this first post.
        if failureCode == nil, acquireVerified, let targetFocus {
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
            sampleAfter("after-down", postedNs: downPostedNs, expectedFocus: targetFocus)
        }

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
                    sampleAfter(
                        "after-\(event.label)", postedNs: postedNs, expectedFocus: targetFocus
                    )
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
        if downAttempts == 1 {
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
            sampleAfter("after-up", postedNs: upPostedNs, expectedFocus: targetFocus)
            if let observedBounds = try? exactWindow(id: window, owner: pid),
               !sameRect(observedBounds, frozenBounds) {
                sameIdentity = false
                recordFailure(
                    "window_geometry_changed", "the exact target geometry changed during the gesture"
                )
            } else if (try? exactWindow(id: window, owner: pid)) == nil {
                sameIdentity = false
                recordFailure(
                    "window_identity_changed", "the exact target identity changed during the gesture"
                )
            }
        }
        restorePeer()
    }

    if mode == .releaseOnly && (try? exactWindow(id: window, owner: pid)) == nil {
        sameIdentity = false
        recordFailure("window_identity_changed", "the exact target identity changed during release recovery")
    }

    let expectedSamples: Int
    switch mode {
    case .gesture:
        expectedSamples = downAttempts == 1 ? moveAttempts + 6 : 4
    case .releaseOnly:
        expectedSamples = 3
    case .leaseOnly:
        expectedSamples = 3
    }
    let hostSamplesComplete = samples.count == expectedSamples
    if !hostSamplesComplete {
        recordFailure("host_samples_incomplete", "an intermediate host-state sample is missing")
    }
    let ok = failureCode == nil && acquireVerified == (mode != .releaseOnly)
        && restoreVerified
    let outcomeUnknown = mode == .releaseOnly || downPostUncertain
        || upPostUncertain || (downAttempts == 1 && upAttempts != 1) || !restoreVerified
        || (downAttempts > 0 && failureCode != nil)
    let unavailableFocusFields = (priorFocus?.unavailableFields("prior") ?? [])
        + (targetFocus?.unavailableFields("target") ?? [])
    let notSampledFocusFields = ["application.key_window"]
    return [
        "ok": ok && hostSamplesComplete && sameIdentity,
        "schema": 2,
        "mode": mode.rawValue,
        "arm": route.rawValue,
        "pid": pid,
        "window": window,
        "peer_window": peerWindow,
        "guard_window": guardWindow,
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
        "down_post_uncertain": downPostUncertain,
        "up_attempted": upAttempts == 1,
        "up_post_uncertain": upPostUncertain,
        "up_proven": upAttempts == 1 && !upPostUncertain,
        "up_proof": upAttempts == 1 && !upPostUncertain
            ? "same-route-native-post-returned" : "unproved",
        "same_identity": sameIdentity,
        "host_samples_complete": hostSamplesComplete,
        "host_unchanged": samples.allSatisfy { $0.sameG6a(as: baseline) },
        "max_post_to_sample_ms": Double(maxPostToSampleNs) / 1_000_000.0,
        "host_samples": samples.map(\.json),
        "focus_lease": [
            "prior": priorFocus?.json as Any? ?? NSNull(),
            "target": targetFocus?.json as Any? ?? NSNull(),
            "unavailable_fields": unavailableFocusFields,
            "not_sampled_fields": notSampledFocusFields,
            "acquire": [
                "attempted": acquireAttempted,
                "verified": acquireVerified,
                "readback_delay_ms": Int(focusReadbackDelayMicroseconds / 1_000),
                "readback": acquireReadback?.json as Any? ?? NSNull(),
                "started_ns": acquireStartedNs as Any? ?? NSNull(),
                "verified_ns": acquireVerifiedNs as Any? ?? NSNull(),
            ],
            "restore": [
                "attempted": restoreAttempted,
                "verified": restoreVerified,
                "readback_delay_ms": Int(focusReadbackDelayMicroseconds / 1_000),
                "readback": restoreReadback?.json as Any? ?? NSNull(),
                "started_ns": restoreStartedNs as Any? ?? NSNull(),
                "verified_ns": restoreVerifiedNs as Any? ?? NSNull(),
            ],
            "phase_handshakes": phaseReceipts.map(\.json),
            "z_order_phases": samples.map {
                ["stage": $0.stage, "t_ns": $0.tNs, "digest": $0.zOrderDigest]
            },
        ],
        "outcome_unknown": outcomeUnknown,
        "retry_safe": !outcomeUnknown,
        "outcome": ok
            ? (mode == .leaseOnly ? "focus-lease-verified"
                : mode == .releaseOnly ? "release-posted-focus-restored-awaiting-oracle"
                : "posted-focus-restored-awaiting-oracle")
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
        "schema": 2,
        "down_attempts": 0,
        "move_attempts": 0,
        "up_attempts": 0,
        "down_post_uncertain": false,
        "up_attempted": false,
        "up_post_uncertain": false,
        "up_proven": false,
        "same_identity": false,
        "host_samples_complete": false,
        "host_unchanged": false,
        "max_post_to_sample_ms": 0,
        "outcome_unknown": false,
        "retry_safe": true,
        "outcome": "failed-before-down",
        "focus_lease": NSNull(),
        "error": ["code": code, "message": message],
    ])
    exit(1)
} catch {
    writeJSON([
        "ok": false,
        "schema": 2,
        "down_attempts": 0,
        "move_attempts": 0,
        "up_attempts": 0,
        "down_post_uncertain": false,
        "up_attempted": false,
        "up_post_uncertain": false,
        "up_proven": false,
        "same_identity": false,
        "host_samples_complete": false,
        "host_unchanged": false,
        "max_post_to_sample_ms": 0,
        "outcome_unknown": false,
        "retry_safe": true,
        "outcome": "failed-before-down",
        "focus_lease": NSNull(),
        "error": ["code": "injector_failed", "message": "the injector failed before the button-down"],
    ])
    exit(1)
}
