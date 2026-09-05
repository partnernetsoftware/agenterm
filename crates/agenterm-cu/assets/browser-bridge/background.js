"use strict";

const HOST = "software.partnernet.agenterm_acu.browser_bridge";
const PROTOCOL = 1;
const LIMITS = Object.freeze({ frames: 64, depth: 20, scan: 5000, results: 1000 });
const TAB_LIMITS = Object.freeze({ results: 512, titleCharacters: 1024, urlCharacters: 2048 });
let port = null;

function hasControl(value) {
  return typeof value === "string" && /[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function boundedInteger(value, maximum) {
  return Number.isInteger(value) && value >= 1 && value <= maximum;
}

function validateRequest(request) {
  if (!request || request.protocol !== PROTOCOL || typeof request.id !== "string" ||
      !/^[A-Za-z0-9._:-]{1,96}$/u.test(request.id) ||
      !request.args || Array.isArray(request.args) || typeof request.args !== "object") {
    throw new Error("browser_bridge_request_invalid");
  }
  if (!["status", "tabs", "debug-read"].includes(request.command)) {
    throw new Error("browser_bridge_command_unknown");
  }
  if (request.command !== "debug-read" && Object.keys(request.args).length !== 0) {
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

async function dispatch(request) {
  validateRequest(request);
  if (request.command === "status") {
    return { protocol: PROTOCOL, extension_id: chrome.runtime.id, commands: ["status", "tabs", "debug-read"] };
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
  return debugRead(request.args);
}

function connect() {
  if (port) return;
  port = chrome.runtime.connectNative(HOST);
  port.onMessage.addListener(async request => {
    try {
      port.postMessage({ protocol: PROTOCOL, id: request && request.id,
        ok: true, result: await dispatch(request) });
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
