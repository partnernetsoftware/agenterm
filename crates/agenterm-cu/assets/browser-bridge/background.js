"use strict";

const HOST = "software.partnernet.agenterm_acu.browser_bridge";
const PROTOCOL = 6;
const BUILD_ID = "__ACU_BUILD_ID__";
const PROFILE_INSTANCE_KEY = "acuProfileInstanceId";
// One extra result lets compatibility projections observe the legacy page
// ceiling with a max+1 sentinel instead of reporting a false complete page.
const LIMITS = Object.freeze({ frames: 64, depth: 20, scan: 5000, results: 1001 });
const TAB_LIMITS = Object.freeze({ results: 512, titleCharacters: 1024, urlCharacters: 2048 });
const WINDOW_LIMITS = Object.freeze({ results: 256 });
const DEBUG_ERROR_CODES = new Set([
  "browser_bridge_debug_read_frame_invalid", "browser_bridge_debug_read_frame_limit",
  "browser_bridge_debug_read_limit_invalid", "browser_bridge_debug_read_presentation_changed",
  "browser_bridge_debug_read_detach_failed", "browser_bridge_debug_target_invalid",
  "browser_bridge_debug_action_invalid", "browser_bridge_debug_expectation_invalid",
  "browser_bridge_debug_text_invalid", "browser_bridge_debug_files_invalid",
  "browser_bridge_debug_foreground_refused", "browser_bridge_debug_target_missing",
  "browser_bridge_debug_target_ambiguous", "browser_bridge_debug_target_unresolvable",
  "browser_bridge_debug_file_target_invalid", "browser_bridge_debug_file_multiple_refused",
  "browser_bridge_debug_file_readback_failed", "browser_bridge_debug_type_failed",
  "browser_bridge_debug_type_readback_failed", "browser_bridge_debug_focus_readback_failed",
  "browser_bridge_debug_press_failed", "browser_bridge_debug_press_readback_failed",
  "browser_bridge_debug_press_role_mismatch", "browser_bridge_debug_press_name_mismatch",
  "browser_bridge_debug_presentation_changed", "browser_bridge_debug_detach_failed"
]);
const NAV_ERROR_CODES = new Set([
  "browser_bridge_nav_args_invalid", "browser_bridge_nav_foreground_refused",
  "browser_bridge_nav_tab_invalid", "browser_bridge_nav_frame_invalid",
  "browser_bridge_nav_event_limit", "browser_bridge_nav_actuation_failed",
  "browser_bridge_nav_commit_timeout", "browser_bridge_nav_dialog_blocked",
  "browser_bridge_nav_failed", "browser_bridge_nav_postcondition_failed",
  "browser_bridge_nav_presentation_changed", "browser_bridge_nav_detach_failed"
]);
const EXTENSION_RELOAD_ERROR_CODES = new Set([
  "browser_bridge_extension_reload_args_invalid",
  "browser_bridge_extension_reload_identity_changed",
  "browser_bridge_extension_reload_actuation_failed"
]);
const NAV_COMMIT_TIMEOUT_MS = 20_000;
const NAV_EVENT_MAX = 128;
let port = null;
let profileInstancePromise = null;
let connectPromise = null;

function randomProfileInstanceId() {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  if (bytes.every(byte => byte === 0)) bytes[0] = 1;
  return Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("");
}

function validProfileInstanceId(value) {
  return typeof value === "string" && /^[0-9a-f]{32}$/u.test(value) && !/^0+$/u.test(value);
}

async function profileInstanceId() {
  if (!profileInstancePromise) {
    profileInstancePromise = (async () => {
      const stored = await chrome.storage.local.get(PROFILE_INSTANCE_KEY);
      const existing = stored && stored[PROFILE_INSTANCE_KEY];
      if (validProfileInstanceId(existing)) return existing;
      const created = randomProfileInstanceId();
      await chrome.storage.local.set({ [PROFILE_INSTANCE_KEY]: created });
      const verified = (await chrome.storage.local.get(PROFILE_INSTANCE_KEY))[PROFILE_INSTANCE_KEY];
      if (verified !== created) throw new Error("browser_bridge_profile_identity_publish_failed");
      return created;
    })().catch(error => {
      profileInstancePromise = null;
      throw error;
    });
  }
  return profileInstancePromise;
}

function hasControl(value) {
  return typeof value === "string" && /[\u0000-\u001f\u007f-\u009f]/u.test(value);
}

function boundedInteger(value, maximum) {
  return Number.isInteger(value) && value >= 1 && value <= maximum;
}

function validNavArgs(args) {
  if (!args || Array.isArray(args) || typeof args !== "object" ||
      Object.keys(args).sort().join(",") !== "tab_id,url" ||
      !boundedInteger(args.tab_id, 0xffffffff) || typeof args.url !== "string" ||
      !/^[\x21-\x7e]{1,2048}$/u.test(args.url)) return false;
  try {
    const parsed = new URL(args.url);
    return (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      typeof parsed.host === "string" && parsed.host.length > 0 &&
      parsed.username.length === 0 && parsed.password.length === 0;
  } catch (_) {
    return false;
  }
}

function beforeDeadline(promise, deadline, code) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) return Promise.reject(new Error(code));
  let timer = null;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(code)), remaining);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
}

function signedInteger(value) {
  return Number.isInteger(value) && value >= -0x80000000 && value <= 0x7fffffff;
}

function debugErrorCode(error, fallback) {
  const code = String(error && error.message || "");
  return DEBUG_ERROR_CODES.has(code) ? code : fallback;
}

function validateRequest(request) {
  if (!request || request.protocol !== PROTOCOL || typeof request.id !== "string" ||
      !/^[A-Za-z0-9._:-]{1,96}$/u.test(request.id) ||
      !request.args || Array.isArray(request.args) || typeof request.args !== "object") {
    throw new Error("browser_bridge_request_invalid");
  }
  if (!["status", "tabs", "windows", "window-open", "window-state", "nav", "debug-read", "debug-invoke", "debug-type", "debug-files", "reload", "extension-reload"].includes(request.command)) {
    throw new Error("browser_bridge_command_unknown");
  }
  if (!["status", "debug-read", "debug-invoke", "debug-type", "debug-files", "window-open", "window-state", "nav", "reload", "extension-reload"].includes(request.command) && Object.keys(request.args).length !== 0) {
    throw new Error("browser_bridge_args_invalid");
  }
  if (request.command === "status") {
    const keys = Object.keys(request.args).sort().join(",");
    if (keys !== "" && (keys !== "reload_identity" || request.args.reload_identity !== true)) {
      throw new Error("browser_bridge_args_invalid");
    }
  }
  if (request.command === "nav" && !validNavArgs(request.args)) {
    throw new Error("browser_bridge_nav_args_invalid");
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

const DEBUG_ACTIONABLE_ROLES = new Set([
  "button", "checkbox", "combobox", "link", "listbox", "menuitem", "radio",
  "searchbox", "slider", "spinbutton", "switch", "tab", "textbox", "treeitem"
]);

function axPropertyBoolean(node, name) {
  for (const property of Array.isArray(node.properties) ? node.properties.slice(0, 128) : []) {
    if (property && property.name === name && property.value &&
        typeof property.value.value === "boolean") return property.value.value;
  }
  return false;
}

function debugNodeActionable(node, role) {
  return DEBUG_ACTIONABLE_ROLES.has(role.toLowerCase()) ||
    axPropertyBoolean(node, "focusable") || axPropertyBoolean(node, "editable");
}

function projectAxNodes(rawNodes, frameId, request, scanBudget, resultBudget) {
  const capped = rawNodes.slice(0, scanBudget);
  const byId = new Map(capped.filter(node => node && typeof node.nodeId === "string")
    .map(node => [node.nodeId, node]));
  const depths = new Map();
  function depthOf(node) {
    if (depths.has(node.nodeId)) return depths.get(node.nodeId);
    let depth = 0;
    let parentId = node.parentId;
    const seen = new Set([node.nodeId]);
    while (typeof parentId === "string" && byId.has(parentId) && !seen.has(parentId) && depth <= request.max_depth) {
      seen.add(parentId);
      depth += 1;
      parentId = byId.get(parentId).parentId;
    }
    depths.set(node.nodeId, depth);
    return depth;
  }
  const result = [];
  let scanned = 0;
  let textTruncated = false;
  let resultTruncated = false;
  for (const node of capped) {
    scanned += 1;
    const depth = depthOf(node);
    if (depth > request.max_depth || !Number.isSafeInteger(node.backendDOMNodeId) ||
        node.backendDOMNodeId < 1) continue;
    const rawRole = node.role && typeof node.role.value === "string" ? node.role.value : "";
    const role = boundedText(rawRole, 64);
    const actionable = debugNodeActionable(node, rawRole);
    if (request.actionable && !actionable) continue;
    if (result.length >= resultBudget) {
      resultTruncated = true;
      continue;
    }
    const name = boundedText(node.name && node.name.value, 4096);
    textTruncated ||= role.truncated || name.truncated;
    result.push({
      frame_id: frameId,
      backend_node_id: node.backendDOMNodeId,
      depth,
      role: role.text || "node",
      name: name.text,
      actionable,
      disabled: axPropertyBoolean(node, "disabled"),
      focused: axPropertyBoolean(node, "focused")
    });
  }
  return { nodes: result, scanned,
    truncated: textTruncated || capped.length < rawNodes.length || resultTruncated };
}

async function debugRead(args) {
  const keys = Object.keys(args).sort().join(",");
  if ((keys !== "actionable,max_depth,max_frames,max_results,max_scan,tab_id" &&
       keys !== "max_depth,max_frames,max_results,max_scan,tab_id") ||
      !boundedInteger(args.tab_id, 0x7fffffff) ||
      !boundedInteger(args.max_frames, LIMITS.frames) ||
      !boundedInteger(args.max_depth, LIMITS.depth) ||
      !boundedInteger(args.max_scan, LIMITS.scan) ||
      !boundedInteger(args.max_results, LIMITS.results) ||
      (args.actionable !== undefined && typeof args.actionable !== "boolean")) {
    throw new Error("browser_bridge_debug_read_limit_invalid");
  }
  args.actionable = args.actionable === true;
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
      if (flattened.scanned >= args.max_scan) {
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
      request_actionable: args.actionable,
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
    readError = debugErrorCode(error, "browser_bridge_debug_read_failed");
  } finally {
    if (attached) {
      try {
        await chrome.debugger.detach(target);
        detach = { outcome: "detached" };
      } catch (error) {
        detach = { outcome: "failed", code: "browser_bridge_debug_detach_failed" };
      }
    }
  }
  if (detach.outcome === "failed" && !readError) {
    readError = "browser_bridge_debug_read_detach_failed";
  }
  if (readError) return { tab_id: args.tab_id, code: readError, detach };
  return { ...result, detach };
}

function scalarAxValue(value) {
  const scalar = value && typeof value === "object" ? value.value : undefined;
  return typeof scalar === "string" || typeof scalar === "number" || typeof scalar === "boolean"
    ? scalar : undefined;
}

function exactDebugTarget(nodes, args) {
  const matches = (Array.isArray(nodes) ? nodes : []).filter(node =>
    node && node.ignored !== true && node.backendDOMNodeId === args.backend_node_id &&
    String(scalarAxValue(node.role) ?? "") === args.role &&
    String(scalarAxValue(node.name) ?? "") === args.name);
  if (matches.length !== 1) throw new Error(matches.length === 0
    ? "browser_bridge_debug_target_missing" : "browser_bridge_debug_target_ambiguous");
  return matches[0];
}

function targetDescriptor(args) {
  return { frame_id: args.frame_id, backend_node_id: args.backend_node_id,
    role: args.role, name: args.name };
}

function validateDebugTargetArgs(args, extraKeys) {
  const expected = ["backend_node_id", "frame_id", "name", "role", "tab_id", ...extraKeys].sort().join(",");
  if (Object.keys(args).sort().join(",") !== expected ||
      !boundedInteger(args.tab_id, 0x7fffffff) ||
      !boundedInteger(args.backend_node_id, Number.MAX_SAFE_INTEGER) ||
      typeof args.frame_id !== "string" || !/^[A-Za-z0-9._:-]{1,256}$/u.test(args.frame_id) ||
      typeof args.role !== "string" || args.role.length < 1 || Array.from(args.role).length > 64 || hasControl(args.role) ||
      typeof args.name !== "string" || Array.from(args.name).length > 4096 || hasControl(args.name)) {
    throw new Error("browser_bridge_debug_target_invalid");
  }
}

function axProperty(node, name) {
  const property = (Array.isArray(node && node.properties) ? node.properties : [])
    .find(candidate => candidate && candidate.name === name);
  return scalarAxValue(property && property.value);
}

async function debugActuate(command, args) {
  const extraKeys = command === "debug-invoke"
    ? ["action", "expect_name", "expect_role"]
    : command === "debug-type" ? ["text"] : ["files"];
  validateDebugTargetArgs(args, extraKeys);
  if (command === "debug-invoke" && !["focus", "press"].includes(args.action)) {
    throw new Error("browser_bridge_debug_action_invalid");
  }
  if (command === "debug-invoke") {
    const expects = args.action === "press";
    if (typeof args.expect_role !== "string" || typeof args.expect_name !== "string" ||
        (expects && (args.expect_role.length < 1 || args.expect_name.length < 1)) ||
        (!expects && (args.expect_role.length !== 0 || args.expect_name.length !== 0)) ||
        Array.from(args.expect_role).length > 64 || Array.from(args.expect_name).length > 4096 ||
        hasControl(args.expect_role) || hasControl(args.expect_name)) {
      throw new Error("browser_bridge_debug_expectation_invalid");
    }
  }
  if (command === "debug-type" && (typeof args.text !== "string" ||
      new TextEncoder().encode(args.text).length > 65536)) {
    throw new Error("browser_bridge_debug_text_invalid");
  }
  if (command === "debug-files" && (!Array.isArray(args.files) || args.files.length < 1 ||
      args.files.length > 32 || args.files.some(file => !file || typeof file.path !== "string" ||
        file.path.length < 1 || file.path.length > 4096 || file.path.includes("\u0000") ||
        typeof file.name !== "string" || file.name.length < 1 || Array.from(file.name).length > 4096 ||
        file.name.includes("\u0000") || !Number.isSafeInteger(file.size) || file.size < 0))) {
    throw new Error("browser_bridge_debug_files_invalid");
  }

  const browserTarget = { tabId: args.tab_id };
  let attached = false;
  let detach = { outcome: "already-detached" };
  let result;
  let actuationError = null;
  let effectStarted = false;
  try {
    const before = await tabPresentation(args.tab_id);
    if (before.focused) throw new Error("browser_bridge_debug_foreground_refused");
    await chrome.debugger.attach(browserTarget, "1.3");
    attached = true;
    detach = { outcome: "failed", code: "detach_not_attempted" };
    await chrome.debugger.sendCommand(browserTarget, "Accessibility.enable");
    const tree = await chrome.debugger.sendCommand(browserTarget, "Accessibility.getFullAXTree", {
      frameId: args.frame_id, depth: LIMITS.depth
    });
    const node = exactDebugTarget(tree.nodes, args);
    const target = targetDescriptor(args);
    if (command === "debug-files") {
      const resolved = await chrome.debugger.sendCommand(browserTarget, "DOM.resolveNode", {
        backendNodeId: args.backend_node_id
      });
      const objectId = resolved && resolved.object && resolved.object.objectId;
      if (typeof objectId !== "string") throw new Error("browser_bridge_debug_target_unresolvable");
      const preflight = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
        objectId, returnByValue: true,
        functionDeclaration: "function() { return { fileInput: this instanceof HTMLInputElement && this.type === 'file', multiple: this.multiple === true, disabled: this.disabled === true || this.getAttribute('aria-disabled') === 'true' }; }"
      });
      const state = preflight && preflight.result && preflight.result.value;
      if (!state || state.fileInput !== true || state.disabled === true) {
        throw new Error("browser_bridge_debug_file_target_invalid");
      }
      if (args.files.length > 1 && state.multiple !== true) {
        throw new Error("browser_bridge_debug_file_multiple_refused");
      }
      effectStarted = true;
      await chrome.debugger.sendCommand(browserTarget, "DOM.setFileInputFiles", {
        backendNodeId: args.backend_node_id, files: args.files.map(file => file.path)
      });
      const verify = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
        objectId, returnByValue: true,
        functionDeclaration: "function() { return Array.from(this.files, file => ({ name: file.name, size: file.size })); }"
      });
      const files = verify && verify.result && verify.result.value;
      if (!Array.isArray(files) || files.length !== args.files.length || files.some((file, index) =>
        !file || file.name !== args.files[index].name || file.size !== args.files[index].size)) {
        throw new Error("browser_bridge_debug_file_readback_failed");
      }
      result = { tab_id: args.tab_id, target, action: "files", file_count: files.length,
        files, multiple: state.multiple === true, performed: true, verified: true };
    } else if (command === "debug-type") {
      const resolved = await chrome.debugger.sendCommand(browserTarget, "DOM.resolveNode", {
        backendNodeId: args.backend_node_id
      });
      const objectId = resolved && resolved.object && resolved.object.objectId;
      if (typeof objectId !== "string") throw new Error("browser_bridge_debug_target_unresolvable");
      effectStarted = true;
      const written = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
        objectId, returnByValue: true, awaitPromise: true, arguments: [{ value: args.text }],
        functionDeclaration: `function(text) {
          let prototype = this; let setter;
          while (prototype && !setter) {
            const descriptor = Object.getOwnPropertyDescriptor(prototype, "value");
            if (descriptor && typeof descriptor.set === "function") setter = descriptor.set;
            prototype = Object.getPrototypeOf(prototype);
          }
          if (setter) setter.call(this, text);
          else if (this.isContentEditable) this.textContent = text;
          else throw new Error("target is not editable");
          this.dispatchEvent(new InputEvent("input", { bubbles: true, inputType: "insertText", data: text }));
          this.dispatchEvent(new Event("change", { bubbles: true }));
          return true;
        }`
      });
      if (!written || written.exceptionDetails || written.result.value !== true) {
        throw new Error("browser_bridge_debug_type_failed");
      }
      const verify = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
        objectId, returnByValue: true, arguments: [{ value: args.text }],
        functionDeclaration: "function(expected) { return this.isContentEditable ? this.textContent === expected : this.value === expected; }"
      });
      if (!verify || !verify.result || verify.result.value !== true) {
        throw new Error("browser_bridge_debug_type_readback_failed");
      }
      result = { tab_id: args.tab_id, target, action: "type",
        text_utf8_bytes: new TextEncoder().encode(args.text).length, performed: true, verified: true };
    } else if (args.action === "focus") {
      effectStarted = true;
      await chrome.debugger.sendCommand(browserTarget, "DOM.focus", { backendNodeId: args.backend_node_id });
      const afterTree = await chrome.debugger.sendCommand(browserTarget, "Accessibility.getFullAXTree", {
        frameId: args.frame_id, depth: LIMITS.depth
      });
      const afterNode = exactDebugTarget(afterTree.nodes, args);
      if (axProperty(afterNode, "focused") !== true) throw new Error("browser_bridge_debug_focus_readback_failed");
      result = { tab_id: args.tab_id, target, action: "focus", focused: true,
        performed: true, verified: true };
    } else {
      const resolved = await chrome.debugger.sendCommand(browserTarget, "DOM.resolveNode", {
        backendNodeId: args.backend_node_id
      });
      const objectId = resolved && resolved.object && resolved.object.objectId;
      if (typeof objectId !== "string") throw new Error("browser_bridge_debug_target_unresolvable");
      effectStarted = true;
      const pressed = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
        objectId, returnByValue: true, arguments: [{ value: args.expect_name }],
        functionDeclaration: "function(expectedName) { if (this.disabled === true || this.getAttribute('aria-disabled') === 'true') throw new Error('target disabled'); if (typeof this.click !== 'function') throw new Error('target has no click'); this.click(); return { clicked: true, exactTargetPostcondition: this.getAttribute('aria-label') === expectedName }; }"
      });
      const pressValue = pressed && pressed.result && pressed.result.value;
      if (!pressed || pressed.exceptionDetails || !pressValue || pressValue.clicked !== true) {
        throw new Error("browser_bridge_debug_press_failed");
      }
      let expectationPresent = args.expect_role === args.role && pressValue.exactTargetPostcondition === true;
      for (let attempt = 0; attempt < 40 && !expectationPresent; attempt += 1) {
        const targetSemantics = await chrome.debugger.sendCommand(browserTarget, "Runtime.callFunctionOn", {
          objectId, returnByValue: true,
          functionDeclaration: "function() { return { ariaLabel: this.getAttribute('aria-label') || '' }; }"
        });
        const semantics = targetSemantics && targetSemantics.result && targetSemantics.result.value;
        const targetAttributeMatches = args.expect_role === args.role && semantics &&
          semantics.ariaLabel === args.expect_name;
        const targetTree = await chrome.debugger.sendCommand(browserTarget, "Accessibility.getPartialAXTree", {
          backendNodeId: args.backend_node_id, fetchRelatives: false
        });
        const targetMatches = (targetTree.nodes || []).filter(candidate => candidate && candidate.ignored !== true &&
          String(scalarAxValue(candidate.role) ?? "") === args.expect_role &&
          String(scalarAxValue(candidate.name) ?? "") === args.expect_name);
        const afterTree = await chrome.debugger.sendCommand(browserTarget, "Accessibility.getFullAXTree", {
          frameId: args.frame_id, depth: LIMITS.depth
        });
        const matches = (afterTree.nodes || []).filter(candidate => candidate && candidate.ignored !== true &&
          String(scalarAxValue(candidate.role) ?? "") === args.expect_role &&
          String(scalarAxValue(candidate.name) ?? "") === args.expect_name);
        expectationPresent = targetAttributeMatches || targetMatches.length === 1 || matches.length === 1;
        if (!expectationPresent) await new Promise(resolve => setTimeout(resolve, 100));
      }
      if (!expectationPresent) {
        if (args.expect_role !== args.role) throw new Error("browser_bridge_debug_press_role_mismatch");
        if (pressValue.exactTargetPostcondition !== true) throw new Error("browser_bridge_debug_press_name_mismatch");
        throw new Error("browser_bridge_debug_press_readback_failed");
      }
      result = { tab_id: args.tab_id, target, action: "press",
        expectation: { role: args.expect_role, name: args.expect_name, present: true },
        performed: true, verified: true };
    }
    const after = await tabPresentation(args.tab_id);
    if (before.active !== after.active || before.focused !== after.focused) {
      throw new Error("browser_bridge_debug_presentation_changed");
    }
    result.presentation = { tab_active_before: before.active, tab_active_after: after.active,
      window_focused_before: before.focused, window_focused_after: after.focused,
      activation_requested: false };
  } catch (error) {
    actuationError = debugErrorCode(error, "browser_bridge_debug_actuation_failed");
  } finally {
    if (attached) {
      try { await chrome.debugger.detach(browserTarget); detach = { outcome: "detached" }; }
      catch (error) {
        detach = { outcome: "failed", code: "browser_bridge_debug_detach_failed" };
      }
    }
  }
  if (detach.outcome === "failed" && !actuationError) actuationError = "browser_bridge_debug_detach_failed";
  if (actuationError) return { tab_id: args.tab_id, code: actuationError,
    effect: effectStarted ? "unknown" : "not-performed", detach };
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

function windowOpenFailure(error, effect) {
  const failure = new Error(String(error && error.message ||
    "browser_bridge_window_open_failed"));
  failure.effect = effect;
  return failure;
}

function navFailure(error, effect) {
  const raw = String(error && error.message || "browser_bridge_nav_actuation_failed");
  const failure = new Error(NAV_ERROR_CODES.has(raw)
    ? raw : "browser_bridge_nav_actuation_failed");
  failure.effect = effect;
  return failure;
}

async function navigateTab(args) {
  if (!validNavArgs(args)) return { tab_id: args && args.tab_id,
    code: "browser_bridge_nav_args_invalid", effect: "not-performed",
    detach: { outcome: "already-detached" } };
  const target = { tabId: args.tab_id };
  let effectStarted = false;
  let commitProven = false;
  let attached = false;
  let detach = { outcome: "already-detached" };
  let result;
  let navError = null;
  let navEffect = "not-performed";
  let failureNavigation;
  let failureBase;
  const frameNavigations = [];
  const sameDocumentNavigations = [];
  let loadEventFired = false;
  let dialogBlocked = false;
  let navigationEventsOverflow = false;
  let navigationDeadline = 0;
  let rootFrameId = null;
  let signalDialog = null;
  const dialogSignal = new Promise(resolve => { signalDialog = resolve; });
  const onEvent = (source, method, params) => {
    if (!source || source.tabId !== args.tab_id || !params || typeof params !== "object") return;
    if (method === "Page.frameNavigated" && params.frame && typeof params.frame === "object" &&
        params.frame.parentId === undefined) {
      if (frameNavigations.length >= NAV_EVENT_MAX) navigationEventsOverflow = true;
      else frameNavigations.push(params.frame);
    } else if (method === "Page.navigatedWithinDocument" &&
        params.frameId === rootFrameId && params.url === args.url) {
      if (sameDocumentNavigations.length >= NAV_EVENT_MAX) navigationEventsOverflow = true;
      else sameDocumentNavigations.push(params);
    } else if (method === "Page.loadEventFired" && commitProven) {
      loadEventFired = true;
    } else if (method === "Page.javascriptDialogOpening" && effectStarted) {
      dialogBlocked = true;
      signalDialog();
    }
  };
  try {
    const beforeTab = await chrome.tabs.get(args.tab_id);
    if (!beforeTab || beforeTab.id !== args.tab_id ||
        !boundedInteger(beforeTab.windowId, 0xffffffff)) {
      throw new Error("browser_bridge_nav_tab_invalid");
    }
    const beforeWindow = await chrome.windows.get(beforeTab.windowId);
    const beforeActive = beforeTab.active === true;
    const beforeFocused = beforeWindow.focused === true;
    if (beforeFocused) throw new Error("browser_bridge_nav_foreground_refused");

    await chrome.debugger.attach(target, "1.3");
    attached = true;
    detach = { outcome: "failed", code: "detach_not_attempted" };
    await chrome.debugger.sendCommand(target, "Page.enable");
    const beforeFrameTree = await chrome.debugger.sendCommand(target, "Page.getFrameTree");
    const beforeRoot = beforeFrameTree && beforeFrameTree.frameTree &&
      beforeFrameTree.frameTree.frame;
    if (!beforeRoot || typeof beforeRoot.id !== "string" || beforeRoot.id.length < 1 ||
        typeof beforeRoot.loaderId !== "string" || beforeRoot.loaderId.length < 1 ||
        beforeRoot.id.length > 256 || beforeRoot.loaderId.length > 256 ||
        hasControl(beforeRoot.id) || hasControl(beforeRoot.loaderId)) {
      throw new Error("browser_bridge_nav_frame_invalid");
    }
    rootFrameId = beforeRoot.id;
    chrome.debugger.onEvent.addListener(onEvent);
    navigationDeadline = Date.now() + NAV_COMMIT_TIMEOUT_MS;
    effectStarted = true;
    const navigationPromise = chrome.debugger.sendCommand(
      target, "Page.navigate", { url: args.url }).then(value => ({ kind: "navigation", value }));
    navigationPromise.catch(() => {});
    const navigationOutcome = await beforeDeadline(Promise.race([
      navigationPromise,
      dialogSignal.then(() => ({ kind: "dialog" }))
    ]),
      navigationDeadline, "browser_bridge_nav_commit_timeout");
    if (navigationOutcome.kind === "dialog") {
      throw new Error("browser_bridge_nav_dialog_blocked");
    }
    const navigation = navigationOutcome.value;
    if (!navigation || typeof navigation.frameId !== "string" ||
        navigation.frameId.length < 1 || navigation.frameId.length > 256 ||
        hasControl(navigation.frameId) || navigation.frameId !== beforeRoot.id ||
        (navigation.loaderId !== undefined && (typeof navigation.loaderId !== "string" ||
          navigation.loaderId.length < 1 || navigation.loaderId.length > 256 ||
          hasControl(navigation.loaderId)))) {
      throw new Error("browser_bridge_nav_frame_invalid");
    }

    let committedFrame = null;
    let sameDocument = null;
    while (Date.now() < navigationDeadline && !dialogBlocked &&
        committedFrame === null && sameDocument === null) {
      if (navigation.loaderId !== undefined) {
        committedFrame = frameNavigations.find(frame => frame.id === navigation.frameId &&
          frame.loaderId === navigation.loaderId && frame.parentId === undefined) || null;
      } else {
        sameDocument = sameDocumentNavigations.find(event =>
          event.frameId === navigation.frameId && typeof event.url === "string") || null;
      }
      if (committedFrame === null && sameDocument === null) {
        await new Promise(resolve => setTimeout(resolve, 25));
      }
    }
    if (dialogBlocked) throw new Error("browser_bridge_nav_dialog_blocked");
    if (committedFrame === null && sameDocument === null) {
      throw new Error("browser_bridge_nav_commit_timeout");
    }
    const commit = committedFrame || sameDocument;
    const committedRaw = commit.url;
    if (typeof committedRaw !== "string" || committedRaw.length < 1 || hasControl(committedRaw)) {
      throw new Error("browser_bridge_nav_postcondition_failed");
    }
    const committed = boundedText(committedRaw, TAB_LIMITS.urlCharacters);
    const unreachableRaw = committedFrame && typeof committedFrame.unreachableUrl === "string"
      ? committedFrame.unreachableUrl : undefined;
    const unreachable = unreachableRaw === undefined
      ? null : boundedText(unreachableRaw, TAB_LIMITS.urlCharacters);
    const committedLoaderId = navigation.loaderId === undefined
      ? beforeRoot.loaderId : navigation.loaderId;
    failureBase = {
      committed_url: committed.text,
      unreachable_url: unreachable === null ? undefined : unreachable.text,
      frame_id: navigation.frameId,
      loader_id: committedLoaderId
    };
    commitProven = true;
    navEffect = "unknown";
    if (navigationEventsOverflow) throw new Error("browser_bridge_nav_event_limit");
    if ((typeof navigation.errorText === "string" && navigation.errorText.length > 0) ||
        unreachable !== null) {
      failureNavigation = {
        error_text: boundedText(
          typeof navigation.errorText === "string" && navigation.errorText.length > 0
            ? navigation.errorText : "navigation committed an unreachable URL", 1024).text,
        ...failureBase
      };
      throw new Error("browser_bridge_nav_failed");
    }
    const afterTab = await chrome.tabs.get(args.tab_id);
    if (!afterTab || afterTab.id !== args.tab_id || afterTab.windowId !== beforeTab.windowId ||
        typeof afterTab.url !== "string" || hasControl(afterTab.url) ||
        !["loading", "complete"].includes(afterTab.status)) {
      throw new Error("browser_bridge_nav_postcondition_failed");
    }
    const afterWindow = await chrome.windows.get(beforeTab.windowId);
    const afterActive = afterTab.active === true;
    const afterFocused = afterWindow.focused === true;
    if (afterActive !== beforeActive || afterFocused !== beforeFocused) {
      throw new Error("browser_bridge_nav_presentation_changed");
    }
    const observed = boundedText(afterTab.url, TAB_LIMITS.urlCharacters);
    const title = boundedText(afterTab.title, TAB_LIMITS.titleCharacters);
    result = {
      tab_id: args.tab_id,
      requested_url: args.url,
      committed_url: committed.text,
      observed_url: observed.text,
      observed_title: title.text,
      observed_truncated: committed.truncated || observed.truncated || title.truncated ||
        unreachable !== null && unreachable.truncated,
      navigation_kind: committedFrame === null ? "same-document" : "cross-document",
      frame_id: navigation.frameId,
      loader_id: committedLoaderId,
      committed_url_equals_requested: committedRaw === args.url,
      unreachable_url: unreachable === null ? undefined : unreachable.text,
      load_event_fired: loadEventFired,
      load_state: afterTab.status,
      same_document_navigations: sameDocumentNavigations.length,
      performed: true,
      verified: true,
      presentation: {
        tab_active_before: beforeActive,
        tab_active_after: afterActive,
        window_focused_before: beforeFocused,
        window_focused_after: afterFocused,
        activation_requested: false
      }
    };
  } catch (error) {
    const raw = String(error && error.message || "browser_bridge_nav_actuation_failed");
    const performedFailure = commitProven && failureNavigation !== undefined &&
      raw === "browser_bridge_nav_failed";
    const failure = navFailure(error, performedFailure ? "performed" :
      effectStarted ? "unknown" : "not-performed");
    navError = failure.message;
    navEffect = failure.effect;
    if (commitProven && failureNavigation === undefined && failureBase !== undefined) {
      failureNavigation = { error_text: failure.message, ...failureBase };
    }
  } finally {
    chrome.debugger.onEvent.removeListener(onEvent);
    if (attached) {
      try {
        await chrome.debugger.detach(target);
        detach = { outcome: "detached" };
      } catch (_) {
        detach = { outcome: "failed", code: "browser_bridge_nav_detach_failed" };
      }
    }
  }
  if (detach.outcome === "failed" && navError === null) {
    navError = "browser_bridge_nav_detach_failed";
    navEffect = effectStarted ? "unknown" : "not-performed";
    if (commitProven && failureBase !== undefined) {
      failureNavigation = { error_text: navError, ...failureBase };
    }
  }
  if (navError !== null) {
    return { tab_id: args.tab_id, code: navError, effect: navEffect,
      navigation: failureNavigation, detach };
  }
  return { ...result, detach };
}

async function openWindow(args) {
  const keys = Object.keys(args).sort().join(",");
  if (keys !== "focused,url" && keys !== "focused,state,url" ||
      typeof args.focused !== "boolean" ||
      typeof args.url !== "string" || args.url.length < 1 ||
      Array.from(args.url).length > TAB_LIMITS.urlCharacters || hasControl(args.url)) {
    throw new Error("browser_bridge_window_open_args_invalid");
  }
  const state = args.state === undefined ? "normal" : args.state;
  if (!(["normal", "minimized"].includes(state)) ||
      state === "minimized" && args.focused) {
    throw new Error("browser_bridge_window_open_args_invalid");
  }
  const focusedBefore = await focusedWindowId();
  let createdId = null;
  try {
    const created = await chrome.windows.create({
      url: args.url,
      focused: args.focused,
      state,
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
    if (window.state !== state || window.tab_count !== 1 || !validFocus) {
      throw new Error("browser_bridge_window_open_postcondition_failed");
    }
    return {
      requested_focused: args.focused,
      requested_state: state,
      performed: true,
      verified: true,
      focus_changed: focusedBefore !== focusedAfter,
      focused_window_before: focusedBefore === null ? undefined : focusedBefore,
      focused_window_after: focusedAfter === null ? undefined : focusedAfter,
      window
    };
  } catch (error) {
    if (createdId === null) {
      throw windowOpenFailure(error, "unknown");
    }
    try {
      await chrome.windows.remove(createdId);
      if (focusedBefore !== null) await chrome.windows.update(focusedBefore, { focused: true });
      const remaining = await chrome.windows.getAll();
      if (remaining.some(window => window.id === createdId) ||
          await focusedWindowId() !== focusedBefore) {
        throw new Error("browser_bridge_window_open_rollback_failed");
      }
    } catch (_) {
      throw windowOpenFailure(
        new Error("browser_bridge_window_open_rollback_failed"), "unknown");
    }
    throw windowOpenFailure(error, "rolled-back");
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

async function reloadBridge(args) {
  const keys = Object.keys(args).sort().join(",");
  if (keys !== "tab_id" || !boundedInteger(args.tab_id, 0x7fffffff)) {
    throw new Error("browser_bridge_reload_args_invalid");
  }
  const identity = await profileInstanceId();
  const before = await tabPresentation(args.tab_id);
  await chrome.tabs.get(args.tab_id);
  const after = await tabPresentation(args.tab_id);
  if (before.active !== after.active || before.focused !== after.focused) {
    throw new Error("browser_bridge_reload_presentation_changed");
  }
  return { accepted: true, reload_scope: "native-connection", profile_instance_id: identity };
}

async function reloadExtension(args) {
  if (!args || Array.isArray(args) || typeof args !== "object" ||
      Object.keys(args).sort().join(",") !== "before_build_id,before_version,profile_instance_id") {
    throw new Error("browser_bridge_extension_reload_args_invalid");
  }
  const identity = await profileInstanceId();
  const version = chrome.runtime.getManifest().version;
  if (args.profile_instance_id !== identity || args.before_version !== version ||
      args.before_build_id !== BUILD_ID) {
    throw new Error("browser_bridge_extension_reload_identity_changed");
  }
  setTimeout(() => chrome.runtime.reload(), 250);
  return { accepted: true, reload_scope: "extension-code", scheduled: true,
    profile_instance_id: identity, before_version: version, before_build_id: BUILD_ID };
}

async function dispatch(request) {
  validateRequest(request);
  if (request.command === "status") {
    return { protocol: PROTOCOL, extension_id: chrome.runtime.id,
      extension_version: chrome.runtime.getManifest().version,
      build_id: BUILD_ID,
      profile_instance_id: await profileInstanceId(),
      commands: ["status", "tabs", "windows", "window-open", "window-state", "nav", "debug-read", "debug-invoke", "debug-type", "debug-files", "reload", "extension-reload"] };
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
  if (request.command === "nav") return navigateTab(request.args);
  if (["debug-invoke", "debug-type", "debug-files"].includes(request.command)) {
    return debugActuate(request.command, request.args);
  }
  if (request.command === "reload") return reloadBridge(request.args);
  if (request.command === "extension-reload") return reloadExtension(request.args);
  return debugRead(request.args);
}

function connect() {
  if (port) return Promise.resolve();
  if (connectPromise) return connectPromise;
  connectPromise = (async () => {
    await profileInstanceId();
    if (port) return;
    const opened = chrome.runtime.connectNative(HOST);
    port = opened;
    opened.onMessage.addListener(async request => {
      try {
        const result = await dispatch(request);
        if ((request.command.startsWith("debug-") || request.command === "nav") &&
            result && result.code && result.detach) {
          opened.postMessage({ protocol: PROTOCOL, id: request.id, ok: false,
            error: { code: result.code, tab_id: result.tab_id, effect: result.effect,
              navigation: result.navigation, detach: result.detach } });
        } else {
          opened.postMessage({ protocol: PROTOCOL, id: request.id, ok: true, result });
        }
        if (request.command === "reload") {
          setTimeout(() => {
            if (port === opened) port = null;
            opened.disconnect();
            setTimeout(() => { connect().catch(() => {}); }, 50);
          }, 50);
        }
      } catch (error) {
        const isDebug = request && typeof request.command === "string" && request.command.startsWith("debug-");
        const rawCode = String(error && error.message || "browser_bridge_failed")
          .replace(/[\u0000-\u001f\u007f-\u009f]/gu, " ").slice(0, 96);
        const code = isDebug ? debugErrorCode(error, "browser_bridge_debug_failed") :
          request && request.command === "extension-reload"
            ? (EXTENSION_RELOAD_ERROR_CODES.has(rawCode)
              ? rawCode : "browser_bridge_extension_reload_actuation_failed")
            : rawCode;
        const isEffect = request &&
          ["debug-invoke", "debug-type", "debug-files", "window-open", "nav", "extension-reload"].includes(request.command);
        const errorResult = { code: code || "browser_bridge_failed" };
        if (isEffect && isDebug) {
          errorResult.tab_id = request.args && request.args.tab_id;
          errorResult.effect = "not-performed";
          errorResult.detach = { outcome: "already-detached" };
        } else if (request && request.command === "window-open") {
          errorResult.effect = error &&
            ["not-performed", "rolled-back", "unknown"].includes(error.effect)
            ? error.effect : "not-performed";
        } else if (request && request.command === "nav") {
          errorResult.tab_id = request.args && request.args.tab_id;
          errorResult.effect = error && ["not-performed", "performed", "unknown"].includes(error.effect)
            ? error.effect : "not-performed";
          errorResult.navigation = error && error.navigation;
          errorResult.detach = error && error.detach || { outcome: "already-detached" };
        } else if (request && request.command === "extension-reload") {
          errorResult.effect = "not-performed";
        }
        opened.postMessage({ protocol: PROTOCOL, id: request && request.id,
          ok: false, error: errorResult });
      }
    });
    opened.onDisconnect.addListener(() => {
      if (port === opened) port = null;
    });
  })().finally(() => { connectPromise = null; });
  return connectPromise;
}

connect().catch(() => {});
chrome.runtime.onStartup.addListener(() => { connect().catch(() => {}); });
chrome.runtime.onInstalled.addListener(() => { connect().catch(() => {}); });
