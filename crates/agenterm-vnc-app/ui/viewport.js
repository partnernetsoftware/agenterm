// Viewport: the mapping between screen pixels and remote framebuffer pixels.
//
// Pan and zoom are a CSS transform on the canvas rather than a redraw, so the
// bitmap stays at native resolution and this module stays the single authority
// on converting a client point back into server coordinates.

/** Zoom bounds. Below 0.1 the screen is unreadable; above 8 it is one pixel. */
const MIN_SCALE = 0.1;
const MAX_SCALE = 8;

export class Viewport {
  /**
   * @param {HTMLCanvasElement} canvas the surface being transformed
   * @param {HTMLElement} stage the clipping container
   */
  constructor(canvas, stage) {
    this.canvas = canvas;
    this.stage = stage;
    this.scale = 1;
    this.x = 0;
    this.y = 0;
  }

  /** Push the current transform to the DOM. */
  apply() {
    this.canvas.style.transform =
      `translate(${this.x}px, ${this.y}px) scale(${this.scale})`;
  }

  /** Translate by a delta in screen pixels. */
  panBy(dx, dy) {
    this.x += dx;
    this.y += dy;
    this.clamp();
    this.apply();
  }

  /**
   * Scale around a fixed screen point, so the content under a pinch or a
   * cursor stays put instead of drifting toward the origin.
   */
  zoomAt(factor, clientX, clientY) {
    const next = Math.min(MAX_SCALE, Math.max(MIN_SCALE, this.scale * factor));
    if (next === this.scale) return;
    const rect = this.stage.getBoundingClientRect();
    const px = clientX - rect.left;
    const py = clientY - rect.top;
    // Keep the framebuffer point under (px, py) invariant across the change.
    this.x = px - (px - this.x) * (next / this.scale);
    this.y = py - (py - this.y) * (next / this.scale);
    this.scale = next;
    this.clamp();
    this.apply();
  }

  /** Zoom around the centre of the stage, for the HUD buttons. */
  zoomByStep(factor) {
    const rect = this.stage.getBoundingClientRect();
    this.zoomAt(factor, rect.left + rect.width / 2, rect.top + rect.height / 2);
  }

  /** Scale the whole framebuffer to fit, then centre it. */
  fit() {
    const rect = this.stage.getBoundingClientRect();
    const { width, height } = this.canvas;
    if (!width || !height || !rect.width || !rect.height) return;
    this.scale = Math.min(rect.width / width, rect.height / height);
    this.x = (rect.width - width * this.scale) / 2;
    this.y = (rect.height - height * this.scale) / 2;
    this.apply();
  }

  /**
   * Keep the surface from being dragged entirely off screen.
   *
   * An axis smaller than the stage is centred instead of clamped, which is
   * what makes a fitted screen sit in the middle rather than in a corner.
   */
  clamp() {
    const rect = this.stage.getBoundingClientRect();
    const width = this.canvas.width * this.scale;
    const height = this.canvas.height * this.scale;

    this.x = width <= rect.width
      ? (rect.width - width) / 2
      : Math.min(0, Math.max(rect.width - width, this.x));
    this.y = height <= rect.height
      ? (rect.height - height) / 2
      : Math.min(0, Math.max(rect.height - height, this.y));
  }

  /**
   * Convert a client point into framebuffer coordinates.
   *
   * Returns null when the point falls outside the surface, so callers can
   * decline to send a pointer event rather than clamping it to an edge.
   * @returns {{x: number, y: number} | null}
   */
  toRemote(clientX, clientY) {
    const rect = this.stage.getBoundingClientRect();
    const x = Math.floor((clientX - rect.left - this.x) / this.scale);
    const y = Math.floor((clientY - rect.top - this.y) / this.scale);
    if (x < 0 || y < 0 || x >= this.canvas.width || y >= this.canvas.height) return null;
    return { x, y };
  }
}
