#!/usr/bin/env python3
"""Owned, non-activating GTK3 wheel fixture for cu-pointer-scroll-smoke.qjs."""
import json
import os
import signal
import sys

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gdk, Gtk  # noqa: E402

if len(sys.argv) != 2:
    raise SystemExit(2)

state_path = sys.argv[1]
sequence = 0
content_offset = 500.0


def publish():
    temporary = state_path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as stream:
        json.dump({"ready": True, "sequence": sequence, "content_offset": content_offset}, stream)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, state_path)


def on_scroll(_widget, event):
    global sequence, content_offset
    if event.direction == Gdk.ScrollDirection.SMOOTH:
        _ok, _dx, dy = event.get_scroll_deltas()
        content_offset += dy
    elif event.direction == Gdk.ScrollDirection.UP:
        content_offset += 1
    elif event.direction == Gdk.ScrollDirection.DOWN:
        content_offset -= 1
    elif event.direction == Gdk.ScrollDirection.LEFT:
        content_offset += 1
    elif event.direction == Gdk.ScrollDirection.RIGHT:
        content_offset -= 1
    sequence += 1
    publish()
    return True


display = Gdk.Display.get_default()
if display is None:
    raise SystemExit(3)
seat = display.get_default_seat()
pointer = seat.get_pointer()
_screen, pointer_x, pointer_y = pointer.get_position()

window = Gtk.Window(type=Gtk.WindowType.POPUP)
window.set_default_size(240, 180)
window.set_decorated(False)
window.set_keep_above(True)
window.set_accept_focus(False)
window.set_can_focus(False)
window.move(pointer_x - 120, pointer_y - 90)
window.add_events(Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK)
window.connect("scroll-event", on_scroll)
window.connect("destroy", Gtk.main_quit)
window.show_all()
publish()

signal.signal(signal.SIGTERM, lambda _signum, _frame: Gtk.main_quit())
signal.signal(signal.SIGINT, lambda _signum, _frame: Gtk.main_quit())
Gtk.main()
