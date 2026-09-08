#!/usr/bin/env python3
"""Owned GTK3 fixture for Linux secondary-click smoke.

Right-click on a named EventBox opens a popover menu. The context label
gives independent read-back that the secondary press reached the toolkit
handler without --coords degradation.
"""

import os
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import GLib, Gdk, Gtk  # noqa: E402

PID = os.getpid()

settings = Gtk.Settings.get_default()
if settings is not None:
    settings.set_property("gtk-enable-animations", False)

window = Gtk.Window(title="agenterm-linux-rclick-%d" % PID)
window.set_default_size(260, 120)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
context_label = Gtk.Label(label="context idle")
context_area = Gtk.EventBox()
context_area.set_visible_window(True)
context_area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
context_target = Gtk.Label(label="right click here")
context_area.add(context_target)
context_area.get_accessible().set_name("Fixture Right Click")
context_popover = Gtk.Popover()
context_popover.set_relative_to(context_area)
context_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
context_do = Gtk.Button(label="Context Do Thing")
context_disabled = Gtk.Button(label="Context Disabled")
context_disabled.set_sensitive(False)
context_box.add(context_do)
context_box.add(context_disabled)
context_popover.add(context_box)


def on_context_do(_widget):
    context_label.set_text("context did thing")


def on_context_press(_widget, event):
    if event.button == 3:
        context_label.set_text("context menu open")
        context_popover.show()
        return True
    return False


context_area.connect("button-press-event", on_context_press)
context_do.connect("clicked", on_context_do)

for widget in (context_label, context_area):
    box.add(widget)
window.add(box)
window.connect("destroy", Gtk.main_quit)
window.show_all()
window.present()
sys.stdout.write("ready %d\n" % PID)
sys.stdout.flush()
Gtk.main()
