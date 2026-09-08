#!/usr/bin/env python3
"""Disposable GTK fixture for agenterm-cu Linux host-open --app read-back.

One top-level window titled with this process id so an independent `windows`
inventory can bind it after host-open dispatch. The named label gives AT-SPI
get-text --name proof without --coords or screenshot degradation.
"""

import os
import signal
import sys

import gi

gi.require_version("Gtk", "3.0")
from gi.repository import Gtk  # noqa: E402

PID = os.getpid()
TITLE = "agenterm-host-open-fixture-%d" % PID
READY_NAME = "HostOpen Ready"
READY_TEXT = "host-open-ready"


def on_sigterm(_signum, _frame):
    Gtk.main_quit()


signal.signal(signal.SIGTERM, on_sigterm)

window = Gtk.Window(title=TITLE)
window.set_default_size(240, 120)
label = Gtk.Label(label=READY_TEXT)
label.get_accessible().set_name(READY_NAME)
window.add(label)
window.connect("destroy", Gtk.main_quit)
window.show_all()
print("ready %d" % PID, flush=True)
Gtk.main()
