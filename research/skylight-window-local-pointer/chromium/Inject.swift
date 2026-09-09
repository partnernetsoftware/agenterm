import AppKit
import CoreGraphics
import Darwin
import Foundation

private let requiredSymbols = [
    "SLEventPostToPid",
    "SLEventSetIntegerValueField",
    "CGEventSetWindowLocation",
]

private enum InjectorFailure: Error, CustomStringConvertible {
    case typed(String, String)

    var description: String {
        switch self {
        case let .typed(code, message): return "\(code): \(message)"
        }
    }
}

private struct GuardState: Decodable {
    let role: String
    let pid: Int32
}

private struct HostState: Equatable, Encodable {
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

    init() {
        let loaded = dlopen(
            "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
            RTLD_NOW | RTLD_LOCAL
        )
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

    deinit {
        if let handle { dlclose(handle) }
    }

    var available: Bool {
        post != nil && setInteger != nil && setLocation != nil
    }
}

private enum Arm: String {
    case `private`
    case publicLoc = "public-loc"
    case publicOff = "public-off"
}

private func argument(_ name: String) -> String? {
    guard let index = CommandLine.arguments.firstIndex(of: name),
          index + 1 < CommandLine.arguments.count else {
        return nil
    }
    return CommandLine.arguments[index + 1]
}

private func requiredArgument(_ name: String) throws -> String {
    guard let value = argument(name), !value.isEmpty else {
        throw InjectorFailure.typed("argument_missing", "missing \(name)")
    }
    return value
}

private func parseInt32(_ name: String) throws -> Int32 {
    let value = try requiredArgument(name)
    guard let parsed = Int32(value) else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be an i32")
    }
    return parsed
}

private func parseUInt32(_ name: String) throws -> UInt32 {
    let value = try requiredArgument(name)
    guard let parsed = UInt32(value), parsed != 0 else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be a positive u32")
    }
    return parsed
}

private func parseDouble(_ name: String) throws -> Double {
    let value = try requiredArgument(name)
    guard let parsed = Double(value), parsed.isFinite else {
        throw InjectorFailure.typed("argument_invalid", "\(name) must be finite")
    }
    return parsed
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
            "the exact CGWindowID owner does not match"
        )
    }
    guard (row[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
          (row[kCGWindowIsOnscreen as String] as? NSNumber)?.boolValue == true,
          let raw = row[kCGWindowBounds as String] as? [String: CGFloat] else {
        throw InjectorFailure.typed(
            "window_not_onscreen",
            "the exact CGWindowID is not an on-screen layer-zero target"
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
            "the exact CGWindowID has invalid bounds"
        )
    }
    return bounds
}

private func hostState() -> HostState {
    let pointer = NSEvent.mouseLocation
    let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    let rows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements],
        kCGNullWindowID
    ) as? [[String: Any]] ?? []
    let first = rows.first {
        ($0[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == frontmost
            && ($0[kCGWindowLayer as String] as? NSNumber)?.intValue == 0
    }
    return HostState(
        pointerX: pointer.x,
        pointerY: pointer.y,
        frontmostPid: frontmost,
        frontmostWindow: (first?[kCGWindowNumber as String] as? NSNumber)?.uint32Value
    )
}

private func stamp(
    _ event: CGEvent,
    sky: SkyLight,
    pid: pid_t,
    window: UInt32,
    local: CGPoint
) throws {
    guard let setInteger = sky.setInteger, let setLocation = sky.setLocation else {
        throw InjectorFailure.typed(
            "provider_unavailable",
            "required SkyLight symbols are missing"
        )
    }
    let raw = Unmanaged.passUnretained(event).toOpaque()
    // SAFETY: the retained CGEvent owns this object for the complete synchronous
    // native call sequence; no pointer escapes or is stored by the injector.
    for (field, value): (UInt32, Int64) in [
        (0, 2),
        (1, 0),
        (3, 0),
        (7, 3),
        (40, Int64(pid)),
        (51, Int64(window)),
        (58, Int64(DispatchTime.now().uptimeNanoseconds % 1_000_000_000)),
        (91, Int64(window)),
        (92, Int64(window)),
    ] {
        setInteger(raw, field, value)
    }
    setLocation(raw, local.x, local.y)
}

private func writeJSON(_ value: Any) {
    let data = try! JSONSerialization.data(
        withJSONObject: value,
        options: [.sortedKeys]
    )
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
        throw InjectorFailure.typed(
            "provider_unavailable",
            "private SkyLight ABI is not qualified for this OS build and architecture"
        )
    }
}

do {
    try requireMeasuredHost()
    let rawArm = try requiredArgument("--arm")
    guard let arm = Arm(rawValue: rawArm) else {
        throw InjectorFailure.typed(
            "argument_invalid",
            "--arm must be private, public-loc or public-off"
        )
    }
    let pid = try parseInt32("--pid")
    guard pid > 0 else {
        throw InjectorFailure.typed("argument_invalid", "--pid must be positive")
    }
    let window = try parseUInt32("--window")
    let targetPoint = CGPoint(
        x: try parseDouble("--x"),
        y: try parseDouble("--y")
    )
    let offPoint = CGPoint(
        x: try parseDouble("--off-x"),
        y: try parseDouble("--off-y")
    )
    let dx = try parseInt32("--dx")
    let dy = try parseInt32("--dy")
    guard (dx != 0 || dy != 0), dx.magnitude <= 64, dy.magnitude <= 64 else {
        throw InjectorFailure.typed(
            "wheel_delta_out_of_range",
            "wheel deltas must be nonzero and within -64...64"
        )
    }

    let bounds = try exactWindow(id: window, owner: pid)
    guard bounds.contains(targetPoint), !bounds.contains(offPoint) else {
        throw InjectorFailure.typed(
            "control_location_invalid",
            "target point must be inside and off point outside the exact window"
        )
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

    let sky = SkyLight()
    guard sky.available else {
        throw InjectorFailure.typed(
            "provider_unavailable",
            "required SkyLight symbols are missing"
        )
    }
    let location = arm == .publicOff ? offPoint : targetPoint
    guard let event = CGEvent(
        scrollWheelEvent2Source: CGEventSource(stateID: .hidSystemState),
        units: .line,
        wheelCount: 2,
        wheel1: dy,
        wheel2: dx,
        wheel3: 0
    ) else {
        throw InjectorFailure.typed(
            "event_create_failed",
            "CoreGraphics refused the wheel event"
        )
    }
    event.location = location

    let before = hostState()
    switch arm {
    case .private:
        let local = CGPoint(x: targetPoint.x - bounds.minX, y: targetPoint.y - bounds.minY)
        try stamp(event, sky: sky, pid: pid, window: window, local: local)
        guard let post = sky.post else {
            throw InjectorFailure.typed(
                "provider_unavailable",
                "required SkyLight post symbol is missing"
            )
        }
        post(pid, Unmanaged.passUnretained(event).toOpaque())
    case .publicLoc, .publicOff:
        event.postToPid(pid)
    }
    usleep(120_000)
    let after = hostState()

    writeJSON([
        "ok": true,
        "schema": 1,
        "arm": arm.rawValue,
        "pid": pid,
        "window": window,
        "injection_attempts": 1,
        "host_unchanged": before == after,
        "host_before": try JSONSerialization.jsonObject(with: JSONEncoder().encode(before)),
        "host_after": try JSONSerialization.jsonObject(with: JSONEncoder().encode(after)),
        "location": ["x": location.x, "y": location.y],
        "target_bounds": [
            "x": bounds.minX,
            "y": bounds.minY,
            "width": bounds.width,
            "height": bounds.height,
        ],
    ])
} catch let InjectorFailure.typed(code, message) {
    writeJSON([
        "ok": false,
        "schema": 1,
        "error": ["code": code, "message": message],
    ])
    exit(1)
} catch {
    writeJSON([
        "ok": false,
        "schema": 1,
        "error": ["code": "injector_failed", "message": "\(error)"],
    ])
    exit(1)
}
