#!/usr/bin/env python3
"""Owned GTK3 fixture for the agenterm-cu Linux AT-SPI journey.

Two top-level windows so the destructive `close` gate has a victim that is
not the window every other step reads, and a menu bar so the background
menu verbs have something to walk. Every control is named, because a
journey that addresses nodes by `--name` cannot use an unnamed one -- and
an unnamed entry is also what a real application ships when nobody has
thought about accessibility, which is a different test.

Titles carry this process's pid so two runs cannot see each other's
windows. Prints `ready <pid>` once both windows are up, then runs until
SIGTERM.
"""

import os
import sys

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

PID = os.getpid()
presses = [0]
things = [0]

settings = Gtk.Settings.get_default()
if settings is not None:
    settings.set_property("gtk-enable-animations", False)

main_window = Gtk.Window(title="agenterm-linux-fixture-%d" % PID)
main_window.set_default_size(320, 480)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

menubar = Gtk.MenuBar()
file_item = Gtk.MenuItem(label="File")
file_menu = Gtk.Menu()
do_thing = Gtk.MenuItem(label="Do Thing")
disabled_thing = Gtk.MenuItem(label="Disabled Thing")
disabled_thing.set_sensitive(False)
marked_thing = Gtk.CheckMenuItem(label="Marked Thing")
minimize_item = Gtk.MenuItem(label="Minimize")
for item in (do_thing, disabled_thing, marked_thing, minimize_item):
    file_menu.append(item)
file_item.set_submenu(file_menu)
menubar.append(file_item)

# The labels deliberately keep their default accessible name. For a
# GtkLabel that name *is* the displayed text, so overriding it to something
# stable would hide the very value these steps read back.
press_label = Gtk.Label(label="pressed 0")
menu_label = Gtk.Label(label="menu idle")
hover_label = Gtk.Label(label="hover idle")
hover_target = Gtk.Label(label="Fixture Hover")
hover_area = Gtk.EventBox()
hover_area.add(hover_target)
drag_source_entry = Gtk.Entry()
drag_source_entry.set_text("drag source")
drag_source_entry.get_accessible().set_name("Fixture Drag Source")
drag_target_entry = Gtk.Entry()
drag_target_entry.set_text("drag target")
drag_target_entry.get_accessible().set_name("Fixture Drag Target")
wheel_viewport = Gtk.ScrolledWindow()
wheel_viewport.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.ALWAYS)
wheel_viewport.set_size_request(-1, 120)
wheel_rows = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
wheel_rows.add(Gtk.Label(label="wheel row 0"))
wheel_rows.add(Gtk.Label(label="wheel row 1"))
wheel_target = Gtk.Label(label="Fixture Wheel Target")
wheel_rows.add(wheel_target)
for row in range(3, 20):
    wheel_rows.add(Gtk.Label(label="wheel row %d" % row))
scroll_target = Gtk.Label(label="below the fold")
scroll_target.get_accessible().set_name("Fixture Scroll Target")
wheel_rows.add(scroll_target)
wheel_viewport.add(wheel_rows)
entry = Gtk.Entry()
entry.set_text("seed")
entry.get_accessible().set_name("Fixture Entry")
check = Gtk.CheckButton(label="Fixture Check")
button = Gtk.Button(label="Fixture Press")


def on_click(_widget):
    presses[0] += 1
    press_label.set_text("pressed %d" % presses[0])


def on_thing(_widget):
    things[0] += 1
    menu_label.set_text("did thing %d" % things[0])


def on_hover_enter(_widget, _event):
    hover_label.set_text("hover active")


def on_hover_leave(_widget, _event):
    hover_label.set_text("hover idle")


button.connect("clicked", on_click)
do_thing.connect("activate", on_thing)
hover_area.connect("enter-notify-event", on_hover_enter)
hover_area.connect("leave-notify-event", on_hover_leave)
minimize_item.connect("activate", lambda _w: main_window.iconify())

for widget in (
    menubar,
    press_label,
    menu_label,
    hover_label,
    hover_area,
    wheel_viewport,
    drag_source_entry,
    drag_target_entry,
    entry,
    check,
    button,
):
    box.add(widget)
main_window.add(box)
main_window.connect("destroy", Gtk.main_quit)

# The second window is the close gate's victim. It is deliberately plain:
# closing it must not disturb anything the other steps read.
second_window = Gtk.Window(title="agenterm-linux-second-%d" % PID)
second_window.set_default_size(200, 140)
second_label = Gtk.Label(label="second window")
second_window.add(second_label)

second_window.show_all()
main_window.show_all()


def announce():
    # GTK marks a widget STATE_FOCUSED only while its toplevel holds the
    # keyboard focus, so the window the journey reads has to be the one
    # that has it -- the second window was mapped first and would otherwise
    # keep it, leaving the main window's tree with no focused node at all.
    main_window.present()
    sys.stdout.write("ready %d\n" % PID)
    sys.stdout.flush()
    return False


GLib.idle_add(announce)
Gtk.main()
