import AppKit
import CoreGraphics
import Darwin
import Foundation

private let probeSchema = "agenterm.singleton-safety.foreground-probe/v1"

private struct ProbeFailure: Error {
    let code: String
    let exitCode: Int32
}

private struct Snapshot {
    let pid: pid_t
    let bundleID: String
    let handles: [UInt32]
    let totalCount: Int
    let truncated: Bool
}

@discardableResult
private func emit(snapshot: Snapshot?, complete: Bool, errorCode: String?) -> Bool {
    let frontmostApp: Any
    let windows: [String: Any]
    let frontmostHandle: Any
    if let snapshot {
        frontmostApp = [
            "pid": Int(snapshot.pid),
            "bundle_id": snapshot.bundleID,
        ]
        windows = [
            "count": snapshot.totalCount,
            "handles": snapshot.handles.map(Int.init),
            "truncated": snapshot.truncated,
        ]
        frontmostHandle = snapshot.handles.first.map { Int($0) as Any } ?? NSNull()
    } else {
        frontmostApp = NSNull()
        windows = ["count": 0, "handles": [], "truncated": false]
        frontmostHandle = NSNull()
    }
    let error: Any = errorCode.map { ["code": $0] as Any } ?? NSNull()
    let object: [String: Any] = [
        "schema": probeSchema,
        "complete": complete,
        "frontmost_app": frontmostApp,
        "ordinary_onscreen_windows": windows,
        "frontmost_handle": frontmostHandle,
        "error": error,
    ]
    if let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) {
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
        return true
    } else {
        let fallback = "{\"complete\":false,\"error\":{\"code\":\"json_encode_failed\"},\"frontmost_app\":null,\"frontmost_handle\":null,\"ordinary_onscreen_windows\":{\"count\":0,\"handles\":[],\"truncated\":false},\"schema\":\"agenterm.singleton-safety.foreground-probe/v1\"}\n"
        FileHandle.standardOutput.write(Data(fallback.utf8))
        return false
    }
}

private func parseMaximumWindows() throws -> Int {
    let arguments = Array(CommandLine.arguments.dropFirst())
    guard arguments.count == 2, arguments[0] == "--max-windows" else {
        throw ProbeFailure(code: "invalid_arguments", exitCode: 2)
    }
    let raw = arguments[1]
    guard !raw.isEmpty, raw.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }),
          raw.count == 1 || raw.first != "0",
          let value = Int(raw), (1...4096).contains(value) else {
        throw ProbeFailure(code: "max_windows_out_of_range", exitCode: 2)
    }
    return value
}

private func collectSnapshot(maximumWindows: Int) throws -> Snapshot {
    guard let application = NSWorkspace.shared.frontmostApplication,
          !application.isTerminated else {
        throw ProbeFailure(code: "frontmost_application_unavailable", exitCode: 3)
    }
    let pid = application.processIdentifier
    guard pid > 0 else {
        throw ProbeFailure(code: "frontmost_process_invalid", exitCode: 3)
    }
    guard let bundleID = application.bundleIdentifier, !bundleID.isEmpty,
          bundleID.utf8.count <= 256 else {
        throw ProbeFailure(code: "frontmost_bundle_identifier_unavailable", exitCode: 4)
    }
    guard let rows = CGWindowListCopyWindowInfo(
        [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
    ) as? [[String: Any]] else {
        throw ProbeFailure(code: "window_inventory_unavailable", exitCode: 5)
    }

    var allHandles: [UInt32] = []
    var seen: Set<UInt32> = []
    for row in rows {
        guard let owner = row[kCGWindowOwnerPID as String] as? NSNumber else {
            throw ProbeFailure(code: "window_owner_unknown", exitCode: 6)
        }
        if owner.int32Value != pid { continue }
        guard let layer = row[kCGWindowLayer as String] as? NSNumber else {
            throw ProbeFailure(code: "window_layer_unknown", exitCode: 6)
        }
        if layer.intValue != 0 { continue }
        guard let onscreen = row[kCGWindowIsOnscreen as String] as? NSNumber,
              onscreen.boolValue else {
            throw ProbeFailure(code: "window_onscreen_unknown", exitCode: 6)
        }
        guard let number = row[kCGWindowNumber as String] as? NSNumber,
              number.uint64Value > 0, number.uint64Value <= UInt64(UInt32.max) else {
            throw ProbeFailure(code: "window_handle_invalid", exitCode: 6)
        }
        guard let bounds = row[kCGWindowBounds as String] as? NSDictionary else {
            throw ProbeFailure(code: "window_bounds_unknown", exitCode: 6)
        }
        var rectangle = CGRect.zero
        guard CGRectMakeWithDictionaryRepresentation(bounds, &rectangle),
              rectangle.width > 0, rectangle.height > 0 else {
            throw ProbeFailure(code: "window_bounds_invalid", exitCode: 6)
        }
        let handle = number.uint32Value
        guard seen.insert(handle).inserted else {
            throw ProbeFailure(code: "window_handle_duplicate", exitCode: 6)
        }
        allHandles.append(handle)
    }

    let truncated = allHandles.count > maximumWindows
    return Snapshot(
        pid: pid,
        bundleID: bundleID,
        handles: Array(allHandles.prefix(maximumWindows)),
        totalCount: allHandles.count,
        truncated: truncated
    )
}

do {
    let maximumWindows = try parseMaximumWindows()
    let snapshot = try collectSnapshot(maximumWindows: maximumWindows)
    if snapshot.truncated {
        guard emit(snapshot: snapshot, complete: false, errorCode: "window_inventory_truncated") else {
            exit(9)
        }
        exit(7)
    }
    guard emit(snapshot: snapshot, complete: true, errorCode: nil) else { exit(9) }
    exit(0)
} catch let failure as ProbeFailure {
    guard emit(snapshot: nil, complete: false, errorCode: failure.code) else { exit(9) }
    exit(failure.exitCode)
} catch {
    guard emit(snapshot: nil, complete: false, errorCode: "unexpected_probe_failure") else { exit(9) }
    exit(8)
}
