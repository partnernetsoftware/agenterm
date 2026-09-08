#!/usr/bin/env python3
"""Owned GTK3 fixture for wait --window-title-contains Unicode smoke.

Single top-level window whose title includes a combining-accent café token so
case folding is exercised on real AT-SPI inventory titles. Prints
`ready <pid>` once shown, then runs until SIGTERM.
"""

import os
import sys

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

PID = os.getpid()
TITLE = "Caf\u00e9 agenterm-title-wait-%d" % PID

settings = Gtk.Settings.get_default()
if settings is not None:
    settings.set_property("gtk-enable-animations", False)

window = Gtk.Window(title=TITLE)
window.set_default_size(240, 120)
label = Gtk.Label(label="title-wait-ready")
window.add(label)
window.connect("destroy", Gtk.main_quit)
window.show_all()
print("ready %d" % PID, flush=True)
Gtk.main()
