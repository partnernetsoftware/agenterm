"use strict";

const HOST = "software.partnernet.agenterm_acu.browser_bridge";
const PROTOCOL = 1;
const LIMITS = Object.freeze({ frames: 64, depth: 20, scan: 5000, results: 1000 });
const TAB_LIMITS = Object.freeze({ results: 512, titleCharacters: 1024, urlCharacters: 2048 });
const WINDOW_LIMITS = Object.freeze({ results: 256 });
let port = null;

function hasControl(value) {
  return typeof value === "string" && /[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function boundedInteger(value, maximum) {
  return Number.isInteger(value) && value >= 1 && value <= maximum;
}

function signedInteger(value) {
  return Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff;
}

function validateRequest(request) {
  if (!request || request.protocol !== PROTOCOL || typeof request.id !== "string" ||
      !/^[A-Za-z0-9._:-]{1,96}$/u.test(request.id) ||
      !request.args || Array.isArray(request.args) || typeof request.args !== "object") {
    throw new Error("browser_bridge_request_invalid");
  }
  if (!["status", "tabs", "windows", "window-open", "window-state", "debug-read"].includes(request.command)) {
    throw new Error("browser_bridge_command_unknown");
  }
  if (!["debug-read", "window-open", "window-state"].includes(request.command) && Object.keys(request.args).length !== 0) {
    throw new Error("browser_bridge_args_invalid");
  }
}

async function tabPresentation(tabId) {
  const tab = await chrome.tabs.get(tabId);
  const window = await chrome.windows.get(tab.windowId);
  return { active: tab.active, focused: window.focused };
}

function collectFrameIds(frameTree, maximum) {
  const ids = [];
  const pending = [frameTree];
  while (pending.length && ids.length < maximum + 1) {
    const current = pending.pop();
    if (!current || !current.frame || typeof current.frame.id !== "string" ||
        current.frame.id.length < 1 || current.frame.id.length > 256 || hasControl(current.frame.id)) {
      throw new Error("browser_bridge_debug_read_frame_invalid");
    }
    ids.push(current.frame.id);
    for (const child of current.childFrames || []) pending.push(child);
  }
  return ids;
}

function boundedText(value, maximumCharacters) {
  const raw = typeof value === "string" ? value : "";
  const clean = raw.replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ");
  const characters = Array.from(clean);
  return { text: characters.slice(0, maximumCharacters).join(""),
    truncated: characters.length > maximumCharacters || clean !== raw };
}

function projectAxNodes(rawNodes, frameId, request, scanBudget, resultBudget) {
  const depths = new Map();
  const result = [];
  let scanned = 0;
  let textTruncated = false;
  for (const node of rawNodes) {
    if (scanned >= scanBudget || result.length >= resultBudget) break;
    scanned += 1;
    const depth = node.parentId ? (depths.get(node.parentId) ?? request.max_depth) + 1 : 0;
    depths.set(node.nodeId, depth);
    if (depth > request.max_depth || !Number.isSafeInteger(node.backendDOMNodeId) ||
        node.backendDOMNodeId < 1) continue;
    const role = boundedText(node.role && node.role.value, 64);
    const name = boundedText(node.name && node.name.value, 4096);
    textTruncated ||= role.truncated || name.truncated;
    result.push({
      frame_id: frameId,
      backend_node_id: node.backendDOMNodeId,
      depth,
      role: role.text || "node",
      name: name.text
    });
  }
  return { nodes: result, scanned,
    truncated: textTruncated || scanned < rawNodes.length || result.length >= resultBudget };
}

async function debugRead(args) {
  const keys = Object.keys(args).sort().join(",");
  if (keys !== "max_depth,max_frames,max_results,max_scan,tab_id" ||
      !boundedInteger(args.tab_id, 0x7fffffff) ||
      !boundedInteger(args.max_frames, LIMITS.frames) ||
      !boundedInteger(args.max_depth, LIMITS.depth) ||
      !boundedInteger(args.max_scan, LIMITS.scan) ||
      !boundedInteger(args.max_results, LIMITS.results)) {
    throw new Error("browser_bridge_debug_read_limit_invalid");
  }
  const target = { tabId: args.tab_id };
  let detach = { outcome: "already-detached" };
  let attached = false;
  let result;
  let readError = null;
  try {
    const before = await tabPresentation(args.tab_id);
    await chrome.debugger.attach(target, "1.3");
    attached = true;
    detach = { outcome: "failed", code: "detach_not_attempted" };
    const frameTree = await chrome.debugger.sendCommand(target, "Page.getFrameTree");
    const frameIds = collectFrameIds(frameTree.frameTree, args.max_frames);
    if (frameIds.length > args.max_frames) throw new Error("browser_bridge_debug_read_frame_limit");
    const flattened = { nodes: [], scanned: 0, truncated: false };
    for (const frameId of frameIds) {
      if (flattened.scanned >= args.max_scan || flattened.nodes.length >= args.max_results) {
        flattened.truncated = true;
        break;
      }
      const tree = await chrome.debugger.sendCommand(target, "Accessibility.getFullAXTree", {
        depth: args.max_depth,
        frameId
      });
      const projected = projectAxNodes(tree.nodes || [], frameId, args,
        args.max_scan - flattened.scanned, args.max_results - flattened.nodes.length);
      flattened.nodes.push(...projected.nodes);
      flattened.scanned += projected.scanned;
      flattened.truncated ||= projected.truncated;
    }
    const after = await tabPresentation(args.tab_id);
    if (before.active !== after.active || before.focused !== after.focused) {
      throw new Error("browser_bridge_debug_read_presentation_changed");
    }
    result = {
      tab_id: args.tab_id,
      frame_count: frameIds.length,
      scanned: flattened.scanned,
      truncated: flattened.truncated,
      nodes: flattened.nodes,
      presentation: {
        tab_active_before: before.active,
        tab_active_after: after.active,
        window_focused_before: before.focused,
        window_focused_after: after.focused,
        activation_requested: false
      }
    };
  } catch (error) {
    readError = String(error && error.message || "browser_bridge_debug_read_failed")
      .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").slice(0, 96) ||
      "browser_bridge_debug_read_failed";
  } finally {
    if (attached) {
      try {
        await chrome.debugger.detach(target);
        detach = { outcome: "detached" };
      } catch (error) {
        const code = String(error && error.message || "detach_failed").replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").slice(0, 96);
        detach = { outcome: "failed", code: code || "detach_failed" };
      }
    }
  }
  if (detach.outcome === "failed" && !readError) {
    readError = "browser_bridge_debug_read_detach_failed";
  }
  if (readError) return { tab_id: args.tab_id, code: readError, detach };
  return { ...result, detach };
}

function projectWindow(window) {
  if (!window || !Number.isSafeInteger(window.id) || window.id < 1 || window.id > 0xffffffff ||
      !signedInteger(window.left) || !signedInteger(window.top) ||
      !boundedInteger(window.width, 0xffffffff) || !boundedInteger(window.height, 0xffffffff) ||
      !["normal", "minimized", "maximized", "fullscreen", "locked-fullscreen"].includes(window.state)) {
    return null;
  }
  const tabs = Array.isArray(window.tabs) ? window.tabs : [];
  if (tabs.length > 0xffffffff) return null;
  const activeTabs = tabs.filter(tab => tab && tab.active === true &&
    Number.isSafeInteger(tab.id) && tab.id >= 1 && tab.id <= 0xffffffff);
  if (tabs.length > 0 && activeTabs.length !== 1) return null;
  const active = activeTabs[0];
  const title = boundedText(active && active.title, TAB_LIMITS.titleCharacters);
  return {
    row: {
      window_id: window.id,
      state: window.state,
      focused: window.focused === true,
      bounds: { left: window.left, top: window.top, width: window.width, height: window.height },
      tab_count: tabs.length,
      active_tab_id: active ? active.id : undefined,
      active_tab_title: title.text
    },
    truncated: title.truncated
  };
}

async function windowSnapshot(windowId) {
  const projected = projectWindow(await chrome.windows.get(windowId, { populate: true }));
  if (!projected) throw new Error("browser_bridge_window_snapshot_invalid");
  return projected.row;
}

async function focusedWindowId() {
  const focused = (await chrome.windows.getAll()).filter(window => window.focused === true);
  if (focused.length > 1) throw new Error("browser_bridge_window_focus_ambiguous");
  return focused.length === 1 ? focused[0].id : null;
}

async function openWindow(args) {
  const keys = Object.keys(args).sort().join(",");
  if (keys !== "focused,url" || typeof args.focused !== "boolean" ||
      typeof args.url !== "string" || args.url.length < 1 ||
      Array.from(args.url).length > TAB_LIMITS.urlCharacters || hasControl(args.url)) {
    throw new Error("browser_bridge_window_open_args_invalid");
  }
  const focusedBefore = await focusedWindowId();
  let createdId = null;
  try {
    const created = await chrome.windows.create({
      url: args.url,
      focused: args.focused,
      type: "normal"
    });
    if (!created || !boundedInteger(created.id, 0xffffffff)) {
      throw new Error("browser_bridge_window_open_identity_missing");
    }
    createdId = created.id;
    const window = await windowSnapshot(createdId);
    const focusedAfter = await focusedWindowId();
    const validFocus = args.focused
      ? window.focused === true && focusedAfter === createdId
      : window.focused === false && focusedAfter === focusedBefore;
    if (window.state !== "normal" || window.tab_count !== 1 || !validFocus) {
      throw new Error("browser_bridge_window_open_postcondition_failed");
    }
    return {
      requested_focused: args.focused,
      performed: true,
      verified: true,
      focus_changed: focusedBefore !== focusedAfter,
      focused_window_before: focusedBefore === null ? undefined : focusedBefore,
      focused_window_after: focusedAfter === null ? undefined : focusedAfter,
      window
    };
  } catch (error) {
    if (createdId !== null) {
      try {
        await chrome.windows.remove(createdId);
        if (focusedBefore !== null) await chrome.windows.update(focusedBefore, { focused: true });
        const remaining = await chrome.windows.getAll();
        if (remaining.some(window => window.id === createdId) ||
            await focusedWindowId() !== focusedBefore) {
          throw new Error("browser_bridge_window_open_rollback_failed");
        }
      } catch (_) {
        throw new Error("browser_bridge_window_open_rollback_failed");
      }
    }
    throw error;
  }
}

async function waitWindowState(windowId, state) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const current = await windowSnapshot(windowId);
    if (current.state === state) return current;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error("browser_bridge_window_state_timeout");
}

async function updateWindowState(args) {
  const keys = Object.keys(args).sort().join(",");
  if (keys !== "state,window_id" || !boundedInteger(args.window_id, 0xffffffff) ||
      !["normal", "minimized", "maximized"].includes(args.state)) {
    throw new Error("browser_bridge_window_state_args_invalid");
  }
  const before = await windowSnapshot(args.window_id);
  if (before.state === args.state) {
    return { window_id: args.window_id, requested_state: args.state, performed: false,
      verified: true, focus_preserved: true, before, after: before };
  }
  if (before.focused) throw new Error("browser_bridge_window_state_foreground_refused");

  const focusBefore = (await chrome.windows.getAll()).filter(window => window.focused === true);
  if (focusBefore.length > 1 ||
      (focusBefore.length === 1 && !boundedInteger(focusBefore[0].id, 0xffffffff))) {
    throw new Error("browser_bridge_window_state_focus_authority_unavailable");
  }
  const focusId = focusBefore.length === 1 ? focusBefore[0].id : null;
  let changed = false;
  try {
    await chrome.windows.update(args.window_id, { state: args.state });
    changed = true;
    await waitWindowState(args.window_id, args.state);
    const focusAfterEffect = (await chrome.windows.getAll()).filter(window => window.focused === true);
    if (focusId !== null &&
        (focusAfterEffect.length !== 1 || focusAfterEffect[0].id !== focusId)) {
      await chrome.windows.update(focusId, { focused: true });
    }
    const after = await windowSnapshot(args.window_id);
    const focusAfter = (await chrome.windows.getAll()).filter(window => window.focused === true);
    const browserFocusPreserved = focusId === null
      ? focusAfter.length === 0
      : focusAfter.length === 1 && focusAfter[0].id === focusId;
    if (after.state !== args.state || after.focused !== before.focused ||
        after.tab_count !== before.tab_count || after.active_tab_id !== before.active_tab_id ||
        !browserFocusPreserved) {
      throw new Error("browser_bridge_window_state_postcondition_failed");
    }
    return { window_id: args.window_id, requested_state: args.state, performed: true,
      verified: true, focus_preserved: true, before, after };
  } catch (error) {
    try {
      if (changed) {
        await chrome.windows.update(args.window_id, { state: before.state });
        await waitWindowState(args.window_id, before.state);
      }
      if (focusId !== null) await chrome.windows.update(focusId, { focused: true });
      const rolledBack = await windowSnapshot(args.window_id);
      const focusRolledBack = (await chrome.windows.getAll()).filter(window => window.focused === true);
      const browserFocusRolledBack = focusId === null
        ? focusRolledBack.length === 0
        : focusRolledBack.length === 1 && focusRolledBack[0].id === focusId;
      if (rolledBack.state !== before.state || !browserFocusRolledBack) {
        throw new Error("browser_bridge_window_state_rollback_failed");
      }
    } catch (_) {
      throw new Error("browser_bridge_window_state_rollback_failed");
    }
    throw error;
  }
}

async function dispatch(request) {
  validateRequest(request);
  if (request.command === "status") {
    return { protocol: PROTOCOL, extension_id: chrome.runtime.id, commands: ["status", "tabs", "windows", "window-open", "window-state", "debug-read"] };
  }
  if (request.command === "tabs") {
    const tabs = await chrome.tabs.query({});
    let truncated = tabs.length > TAB_LIMITS.results;
    const bounded = [];
    for (const tab of tabs) {
      if (bounded.length >= TAB_LIMITS.results) break;
      if (!Number.isSafeInteger(tab.id) || tab.id < 1 || tab.id > 0xffffffff ||
          !Number.isSafeInteger(tab.windowId) || tab.windowId < 1 || tab.windowId > 0xffffffff) {
        truncated = true;
        continue;
      }
      const title = boundedText(tab.title, TAB_LIMITS.titleCharacters);
      const url = boundedText(tab.url, TAB_LIMITS.urlCharacters);
      truncated ||= title.truncated || url.truncated;
      bounded.push({ tab_id: tab.id, window_id: tab.windowId,
        active: tab.active, title: title.text, url: url.text });
    }
    return { tabs: bounded, truncated };
  }
  if (request.command === "windows") {
    const windows = await chrome.windows.getAll({ populate: true });
    let truncated = windows.length > WINDOW_LIMITS.results;
    const bounded = [];
    for (const window of windows) {
      if (bounded.length >= WINDOW_LIMITS.results) break;
      const projected = projectWindow(window);
      if (!projected) {
        truncated = true;
        continue;
      }
      truncated ||= projected.truncated;
      bounded.push(projected.row);
    }
    return { windows: bounded, truncated };
  }
  if (request.command === "window-open") return openWindow(request.args);
  if (request.command === "window-state") return updateWindowState(request.args);
  return debugRead(request.args);
}

function connect() {
  if (port) return;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(async request => {
    try {
      const result = await dispatch(request);
      if (request.command === "debug-read" && result && result.code && result.detach) {
        port.postMessage({ protocol: PROTOCOL, id: request.id, ok: false,
          error: { code: result.code, tab_id: result.tab_id, detach: result.detach } });
      } else {
        port.postMessage({ protocol: PROTOCOL, id: request.id, ok: true, result });
      }
    } catch (error) {
      const code = String(error && error.message || "browser_bridge_failed")
        .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").slice(0, 96);
      port.postMessage({ protocol: PROTOCOL, id: request && request.id,
        ok: false, error: { code: code || "browser_bridge_failed" } });
    }
  });
  port.onDisconnect.addListener(() => { port = null; });
}

connect();
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
