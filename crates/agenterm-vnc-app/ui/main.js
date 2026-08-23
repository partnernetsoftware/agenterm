// AgenTerm VNC front end.
//
// Responsibilities: draw frames onto the canvas, recognise pointer gestures,
// and translate both into the backend command surface. All protocol work
// happens in Rust; this file never sees an RFB byte.

import { invoke, listen } from "./tauri.js";
import { keysymFor } from "./keysym.js";
import { Viewport } from "./viewport.js";

/** RFB button-mask bits. */
const BUTTON = { NONE: 0, LEFT: 1, MIDDLE: 2, RIGHT: 4, SCROLL_UP: 8, SCROLL_DOWN: 16 };

/** A touch shorter than this and barely moved counts as a tap, not a drag. */
const TAP_MS = 250;
const TAP_SLOP_PX = 10;
/** Wheel notches below this are treated as trackpad inertia, not a click. */
const WHEEL_THRESHOLD = 12;

const STORAGE_KEY = "agenterm-vnc.last-connection";

const stage = document.getElementById("stage");
const canvas = document.getElementById("screen");
const context = canvas.getContext("2d", { alpha: false });
const hud = document.getElementById("hud");
const sheet = document.getElementById("connect-sheet");
const form = document.getElementById("connect-form");
const hostInput = document.getElementById("host");
const portInput = document.getElementById("port");
const usernameInput = document.getElementById("username");
const passwordInput = document.getElementById("password");
const connectButton = document.getElementById("connect-button");
const errorText = document.getElementById("connect-error");
const keyboardTrap = document.getElementById("keyboard-trap");

const viewport = new Viewport(canvas, stage);

/** Live pointers, keyed by pointerId, for multi-touch gesture recognition. */
const pointers = new Map();
/** Gesture bookkeeping for the current touch interaction. */
let gesture = null;
/** Buttons currently held down on the remote, so moves keep them pressed. */
let heldButtons = BUTTON.NONE;
/** Last remote position, so a button release can reuse it. */
let lastRemote = { x: 0, y: 0 };
let connected = false;

/* Rendering ------------------------------------------------------------ */

/** Paint one frame, resizing and refitting when the resolution changes. */
function drawFrame({ width, height, rgba }) {
  const bytes = rgba instanceof Uint8Array ? rgba : new Uint8Array(rgba);
  const resized = canvas.width !== width || canvas.height !== height;
  if (resized) {
    canvas.width = width;
    canvas.height = height;
  }
  // `ImageData` needs a Uint8ClampedArray over the same buffer; this view is
  // free, unlike a copy of a multi-megabyte surface every frame.
  const clamped = new Uint8ClampedArray(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  context.putImageData(new ImageData(clamped, width, height), 0, 0);
  if (resized) viewport.fit();
}

/* Input ---------------------------------------------------------------- */

/** Send a pointer state to the server, ignoring points off the surface. */
function sendMouse(clientX, clientY, buttons) {
  const remote = viewport.toRemote(clientX, clientY);
  if (!remote) return;
  lastRemote = remote;
  invoke("send_mouse", { x: remote.x, y: remote.y, buttons }).catch(reportSessionLoss);
}

/** Press and immediately release a button at a point, i.e. a click. */
async function clickAt(clientX, clientY, button) {
  const remote = viewport.toRemote(clientX, clientY);
  if (!remote) return;
  lastRemote = remote;
  try {
    await invoke("send_mouse", { x: remote.x, y: remote.y, buttons: button });
    await invoke("send_mouse", { x: remote.x, y: remote.y, buttons: BUTTON.NONE });
  } catch (error) {
    reportSessionLoss(error);
  }
}

/** Distance between the two active pointers, for pinch scale. */
function pointerSpread() {
  const [a, b] = [...pointers.values()];
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** Midpoint of the two active pointers, for pan delta and pinch focus. */
function pointerCentre() {
  const [a, b] = [...pointers.values()];
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

stage.addEventListener("pointerdown", (event) => {
  if (!connected) return;
  stage.setPointerCapture(event.pointerId);
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

  if (pointers.size === 1) {
    gesture = {
      mode: "single",
      startX: event.clientX,
      startY: event.clientY,
      startedAt: performance.now(),
      moved: false,
      // A mouse has real buttons, so it bypasses tap heuristics entirely and
      // presses immediately; touch waits to see whether it is a tap or a drag.
      isMouse: event.pointerType === "mouse",
    };
    if (gesture.isMouse) {
      heldButtons = event.button === 2 ? BUTTON.RIGHT : event.button === 1 ? BUTTON.MIDDLE : BUTTON.LEFT;
      sendMouse(event.clientX, event.clientY, heldButtons);
    }
    return;
  }

  if (pointers.size === 2) {
    // A second finger cancels any single-finger interpretation, including a
    // press already sent, so a two-finger tap does not leave a button held.
    if (heldButtons !== BUTTON.NONE) {
      invoke("send_mouse", { x: lastRemote.x, y: lastRemote.y, buttons: BUTTON.NONE })
        .catch(() => {});
      heldButtons = BUTTON.NONE;
    }
    gesture = {
      mode: "dual",
      startedAt: performance.now(),
      startSpread: pointerSpread(),
      lastCentre: pointerCentre(),
      moved: false,
    };
  }
});

stage.addEventListener("pointermove", (event) => {
  if (!connected || !pointers.has(event.pointerId)) return;
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });

  if (gesture?.mode === "single") {
    const far = Math.hypot(event.clientX - gesture.startX, event.clientY - gesture.startY);
    if (far > TAP_SLOP_PX) gesture.moved = true;
    // Move the remote cursor: with a button held this drags, without one it
    // is a plain hover, which is what single-finger drag should feel like.
    sendMouse(event.clientX, event.clientY, heldButtons);
    return;
  }

  if (gesture?.mode === "dual" && pointers.size === 2) {
    const centre = pointerCentre();
    const spread = pointerSpread();
    const dx = centre.x - gesture.lastCentre.x;
    const dy = centre.y - gesture.lastCentre.y;
    if (Math.hypot(dx, dy) > 1 || Math.abs(spread - gesture.startSpread) > TAP_SLOP_PX) {
      gesture.moved = true;
    }
    // Pinch and pan are applied together so a gesture that does both feels
    // continuous rather than snapping between two modes.
    if (gesture.startSpread > 0 && spread > 0) {
      viewport.zoomAt(spread / gesture.startSpread, centre.x, centre.y);
      gesture.startSpread = spread;
    }
    viewport.panBy(dx, dy);
    gesture.lastCentre = centre;
  }
});

function endPointer(event) {
  if (!pointers.has(event.pointerId)) return;
  const wasDual = gesture?.mode === "dual";
  const quick = gesture && performance.now() - gesture.startedAt < TAP_MS;
  const stationary = gesture && !gesture.moved;
  pointers.delete(event.pointerId);

  if (wasDual) {
    // Two-finger tap -> right click, once both fingers are up.
    if (quick && stationary && pointers.size === 0) {
      clickAt(event.clientX, event.clientY, BUTTON.RIGHT);
    }
    if (pointers.size === 0) gesture = null;
    return;
  }

  if (gesture?.mode === "single") {
    if (gesture.isMouse) {
      heldButtons = BUTTON.NONE;
      sendMouse(event.clientX, event.clientY, BUTTON.NONE);
    } else if (quick && stationary) {
      // Single-finger tap -> left click.
      clickAt(event.clientX, event.clientY, BUTTON.LEFT);
    }
  }
  if (pointers.size === 0) gesture = null;
}

stage.addEventListener("pointerup", endPointer);
stage.addEventListener("pointercancel", endPointer);
stage.addEventListener("contextmenu", (event) => event.preventDefault());

// Trackpad and wheel. Ctrl+wheel is the conventional zoom gesture that a
// pinch on a trackpad already reports; plain wheel scrolls the remote.
stage.addEventListener("wheel", (event) => {
  if (!connected) return;
  event.preventDefault();
  if (event.ctrlKey) {
    viewport.zoomAt(event.deltaY < 0 ? 1.1 : 1 / 1.1, event.clientX, event.clientY);
    return;
  }
  if (Math.abs(event.deltaY) < WHEEL_THRESHOLD) return;
  clickAt(event.clientX, event.clientY, event.deltaY < 0 ? BUTTON.SCROLL_UP : BUTTON.SCROLL_DOWN);
}, { passive: false });

/* Keyboard ------------------------------------------------------------- */

function forwardKey(event, down) {
  if (!connected) return;
  const keysym = keysymFor(event);
  if (!keysym) return;
  event.preventDefault();
  invoke("send_key", { keysym, down }).catch(reportSessionLoss);
}

window.addEventListener("keydown", (event) => {
  // Let the connection form receive its own typing.
  if (!sheet.hidden) return;
  forwardKey(event, true);
});
window.addEventListener("keyup", (event) => {
  if (!sheet.hidden) return;
  forwardKey(event, false);
});

/* HUD ------------------------------------------------------------------ */

hud.addEventListener("click", async (event) => {
  const action = event.target.closest("button")?.dataset.action;
  if (!action) return;
  if (action === "zoom-in") viewport.zoomByStep(1.25);
  else if (action === "zoom-out") viewport.zoomByStep(1 / 1.25);
  else if (action === "fit") viewport.fit();
  else if (action === "keyboard") toggleKeyboard(event.target.closest("button"));
  else if (action === "disconnect") await disconnect();
});

/** Summon or dismiss the on-screen keyboard by focusing a hidden input. */
function toggleKeyboard(button) {
  const active = document.activeElement === keyboardTrap;
  if (active) {
    keyboardTrap.blur();
    button.classList.remove("active");
  } else {
    // pointer-events is off so it never intercepts gestures; focus() still
    // works and is what actually raises the keyboard on iOS.
    keyboardTrap.focus();
    button.classList.add("active");
  }
}

/* Session lifecycle ---------------------------------------------------- */

function showError(message) {
  errorText.textContent = message;
  errorText.hidden = false;
}

/** A failed command means the session is gone; return to the sheet. */
function reportSessionLoss(error) {
  if (!connected) return;
  teardown(String(error?.message ?? error));
}

function teardown(message) {
  connected = false;
  heldButtons = BUTTON.NONE;
  pointers.clear();
  gesture = null;
  hud.hidden = true;
  sheet.hidden = false;
  connectButton.disabled = false;
  connectButton.textContent = "Connect";
  if (message) showError(message);
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  errorText.hidden = true;
  connectButton.disabled = true;
  connectButton.textContent = "Connecting…";

  const host = hostInput.value.trim();
  const port = Number(portInput.value);
  try {
    const { width, height } = await invoke("connect", {
      host,
      port,
      username: usernameInput.value,
      password: passwordInput.value,
    });
    canvas.width = width;
    canvas.height = height;
    viewport.fit();
    // Only a working connection is worth remembering; the password is
    // deliberately not stored.
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ host, port, username: usernameInput.value }),
    );
    connected = true;
    sheet.hidden = true;
    hud.hidden = false;
    connectButton.disabled = false;
    connectButton.textContent = "Connect";
  } catch (error) {
    connectButton.disabled = false;
    connectButton.textContent = "Connect";
    showError(String(error?.message ?? error));
  }
});

async function disconnect() {
  try {
    await invoke("disconnect");
  } catch {
    // Already gone; the teardown below is the outcome either way.
  }
  teardown("");
}

/* Wiring --------------------------------------------------------------- */

window.addEventListener("resize", () => {
  if (connected) viewport.fit();
});

// Restore the last host:port so reconnecting is one tap.
try {
  const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
  if (saved?.host) {
    hostInput.value = saved.host;
    portInput.value = saved.port ?? 5900;
    usernameInput.value = saved.username ?? "";
  }
} catch {
  // A corrupt entry just means the fields keep their defaults.
}

await listen("frame-update", (event) => drawFrame(event.payload));
await listen("session-closed", (event) => teardown(String(event.payload)));
