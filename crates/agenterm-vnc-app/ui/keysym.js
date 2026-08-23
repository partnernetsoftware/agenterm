// Browser key identity -> X11 keysym.
//
// RFB speaks X11 keysyms, so every key must be translated. Printable Latin-1
// characters are their own code point, which covers most typing; everything
// else needs the explicit table below.

/** Non-printable and named keys, by KeyboardEvent.key. */
const NAMED = {
  Backspace: 0xff08, Tab: 0xff09, Enter: 0xff0d, Escape: 0xff1b,
  Home: 0xff50, ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53,
  ArrowDown: 0xff54, PageUp: 0xff55, PageDown: 0xff56, End: 0xff57,
  Insert: 0xff63, Delete: 0xffff,
  F1: 0xffbe, F2: 0xffbf, F3: 0xffc0, F4: 0xffc1, F5: 0xffc2, F6: 0xffc3,
  F7: 0xffc4, F8: 0xffc5, F9: 0xffc6, F10: 0xffc7, F11: 0xffc8, F12: 0xffc9,
  Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9, Meta: 0xffe7,
  CapsLock: 0xffe5, NumLock: 0xff7f, ScrollLock: 0xff14,
  Pause: 0xff13, PrintScreen: 0xff61, Menu: 0xff67,
};

// Left/right variants carry distinct keysyms; `location` 2 means the right one.
const RIGHT_VARIANT = { 0xffe1: 0xffe2, 0xffe3: 0xffe4, 0xffe9: 0xffea, 0xffe7: 0xffe8 };

/**
 * Translate a KeyboardEvent into an X11 keysym, or 0 if unmapped.
 * @param {KeyboardEvent} event
 * @returns {number}
 */
export function keysymFor(event) {
  const named = NAMED[event.key];
  if (named !== undefined) {
    return event.location === 2 && RIGHT_VARIANT[named] ? RIGHT_VARIANT[named] : named;
  }

  // A single-character key is a printable character.
  if ([...event.key].length === 1) {
    const code = event.key.codePointAt(0);
    // Latin-1 maps one-to-one onto the low keysym range.
    if (code >= 0x20 && code <= 0xff) return code;
    // Everything above Latin-1 uses the Unicode keysym offset from RFB's
    // keysym rules: 0x01000000 plus the code point.
    return 0x01000000 + code;
  }

  return 0;
}
