-- fleet.lua — Fleet broker wrapper for Lua scripts.
-- Wraps __host.fleet_call(operation_id, params_json) → result_json.
-- API surface aligned with rh's shipped_surfaces / L2 catalog.

local fleet = {}

local function call(op_id, params)
    params = params or "{}"
    if __host.fleet_call == nil then
        return nil
    end
    local result_json = __host.fleet_call(op_id, params)
    local ok, parsed = pcall(function() return std.json.parse(result_json) end)
    if ok then
        return parsed
    end
    return result_json
end

-- ── fleet.tabs ─────────────────────────────────────────────────────────

fleet.tabs = {}

--- List all tabs.
--- @return table[]
function fleet.tabs.list()
    return call("tabs.list")
end

--- Get the active tab.
--- @return table|nil
function fleet.tabs.active()
    return call("tabs.active")
end

--- Set a note on a tab.
---
--- The params key is `tab`, not `tab_id`: `tabs.set-note`'s OperationSpec
--- (`src/operations.rs`, `TAB_NOTE_PARAMETERS`) declares `tab` + `note`, and
--- `validate_fleet_parameters` (`src/client/mod.rs`) refuses any key the spec
--- does not list. Sending `tab_id` answered every invocation with
--- `broker_invalid_arguments: tabs.set-note does not accept parameter tab_id`,
--- so this function had never once worked. The Lua argument name stays
--- `tab_id` — only the wire key changed, so no caller has to be touched.
--- Matches the rh binding, which has always sent `"tab"`
--- (`src/script_fleet.rs`).
--- @param tab_id string
--- @param note string
function fleet.tabs.set_note(tab_id, note)
    return call("tabs.set-note", std.json.stringify({ tab = tab_id, note = note }))
end

-- ── fleet.terminal ─────────────────────────────────────────────────────

fleet.terminal = {}

--- Paste text into terminal.
--- @param text string
function fleet.terminal.paste(text)
    return call("terminal.paste", std.json.stringify({ text = text }))
end

-- ── fleet.ui ───────────────────────────────────────────────────────────

fleet.ui = {}

--- Bootstrap the UI.
function fleet.ui.bootstrap()
    return call("ui.bootstrap")
end

--- Get UI snapshot.
--- @return table
function fleet.ui.snapshot()
    return call("ui.snapshot")
end

--- Get UI deltas since last snapshot.
--- @return table
function fleet.ui.deltas()
    return call("ui.deltas")
end

--- Send a composer message.
--- @param text string
fleet.ui.composer = {}
function fleet.ui.composer.send(text)
    return call("ui.composer.send", std.json.stringify({ text = text }))
end

--- Hello / ping.
function fleet.ui.hello()
    return call("ui.hello")
end

fleet.ui.input = {}

--- Send pointer event.
--- @param x number
--- @param y number
--- @param action string
function fleet.ui.input.pointer(x, y, action)
    return call("ui.input.pointer", std.json.stringify({ x = x, y = y, action = action }))
end

--- Send wheel event.
--- @param delta number
function fleet.ui.input.wheel(delta)
    return call("ui.input.wheel", std.json.stringify({ delta = delta }))
end

fleet.ui.tab = {}

fleet.ui.tab = {}

--- Open a new child tab.
function fleet.ui.tab.new_child()
    return call("ui.tab.new-child")
end

--- Select a tab by id.
---
--- The params key is `tab`, not `id`: `ui.tab.select` declares the shared
--- `TAB_TARGET_PARAMETERS` (`src/operations.rs`), whose single optional
--- parameter is `tab`. Sending `id` was answered with
--- `broker_invalid_arguments: ui.tab.select does not accept parameter id`.
--- Signature unchanged. Note the host still has no Fleet mutation adapter for
--- `ui.tab.select` (`fleet_mutation_command`, `src/client/mod.rs`), so a
--- conformant call now gets as far as `broker_operation_unknown` instead —
--- see plan/design-fleet-binding-gaps.md §5.
--- @param id string
function fleet.ui.tab.select(id)
    return call("ui.tab.select", std.json.stringify({ tab = id }))
end

fleet.ui.tabs = {}

fleet.ui.tabs = {}

--- Hide tabs panel.
function fleet.ui.tabs.hide()
    return call("ui.tabs.hide")
end

--- Show tabs panel.
function fleet.ui.tabs.show()
    return call("ui.tabs.show")
end

--- Toggle tabs panel visibility.
function fleet.ui.tabs.toggle()
    return call("ui.tabs.toggle")
end

--- Set tabs panel width.
--- @param width integer
function fleet.ui.tabs.set_width(width)
    return call("ui.tabs.set-width", std.json.stringify({ width = width }))
end

fleet.ui.tree = {}

fleet.ui.tree = {}

--- Toggle tree panel.
function fleet.ui.tree.toggle()
    return call("ui.tree.toggle")
end

fleet.ui.window = {}

fleet.ui.window = {}

--- Activate the window.
function fleet.ui.window.activate()
    return call("ui.window.activate")
end

-- ── fleet.workspace ─────────────────────────────────────────────────────

fleet.workspace = {}

fleet.workspace = {}

--- Get workspace info.
--- @return table
function fleet.workspace.info()
    return call("workspace.info")
end

--- Shutdown the workspace.
function fleet.workspace.shutdown()
    return call("workspace.shutdown")
end

-- ── fleet.control_center ───────────────────────────────────────────────

fleet.control_center = {}

fleet.control_center = {}

--- Open the control center.
function fleet.control_center.open()
    return call("control-center.open")
end

--- Close the control center.
function fleet.control_center.close()
    return call("control-center.close")
end

--- Get control center snapshot.
--- @return table
function fleet.control_center.snapshot()
    return call("control-center.snapshot")
end

--- Get control center status.
--- @return table
function fleet.control_center.status()
    return call("control-center.status")
end

-- ── fleet.events ────────────────────────────────────────────────────────

fleet.events = {}

fleet.events = {}

--- Read events buffer.
--- @return table
function fleet.events.read()
    return call("events.read")
end

--- Wait for events (blocking).
--- @param timeout_ms number
--- @return table
function fleet.events.wait(timeout_ms)
    return call("events.wait", std.json.stringify({ timeout_ms = timeout_ms }))
end

-- ── fleet.protocol ─────────────────────────────────────────────────────

fleet.protocol = {}

fleet.protocol = {}

--- Get protocol info.
--- @return table
function fleet.protocol.info()
    return call("protocol.info")
end

-- ── fleet.server ────────────────────────────────────────────────────────

fleet.server = {}

fleet.server = {}

--- Kill the server.
function fleet.server.kill()
    return call("server.kill")
end

return fleet
