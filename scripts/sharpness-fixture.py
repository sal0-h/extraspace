#!/usr/bin/env python3
"""Repeatable GTK/GSK text workload, fullscreen on the virtual tablet monitor.

Emits phase and actual snapshot counts as JSON. This is a diagnostic fixture,
not a measurement of a browser/editor or of input-to-display latency.
"""
import json
import argparse
import time

import gi

gi.require_version('Gtk', '4.0')
gi.require_version('Gdk', '4.0')
gi.require_version('Graphene', '1.0')
from gi.repository import Gdk, Gio, GLib, Graphene, Gtk, Pango

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--quick', action='store_true', help='Short correctness/recovery trial')
args = parser.parse_args()
PHASES = ([('static', 3), ('scroll', 12), ('motion', 3), ('recovery', 8)] if args.quick
          else [('static', 12), ('scroll', 64), ('motion', 16), ('recovery', 8)])


def report(**values):
    print(json.dumps(values), flush=True)


def color(value):
    result = Gdk.RGBA()
    result.parse(value)
    return result


class Page(Gtk.Widget):
    def __init__(self, app):
        super().__init__()
        self.app = app
        self.started = time.monotonic()
        self.phase = None
        self.phase_started = 0
        self.snapshots = 0
        self.last_report = self.started
        self.last_count = 0
        self.background = color('#fafafa')
        self.ink = color('#20252b')
        self.blue = color('#135eac')
        self.red = color('#ad2444')
        self.lines = []
        samples = [
            ('Sans 10', 'Small text: The quick brown fox jumps over the lazy dog. 0123456789', self.ink),
            ('Monospace 10', 'const display = { width: 2304, height: 1440, scale: 1.5 };', self.blue),
            ('Monospace 11', 'if (frame.age > budget) requestFreshFrame(); // preserve responsiveness', self.red),
            ('Sans 12', 'Clear text at the same size — reading, scrolling, and moving windows.', self.ink),
        ]
        for n in range(90):
            font, text, ink = samples[n % len(samples)]
            layout = self.create_pango_layout(f'{n:02d}  {text}')
            layout.set_font_description(Pango.FontDescription(font))
            self.lines.append((layout, ink))
        self.title = self.create_pango_layout('Extraspace sharpness comparison • same logical text size')
        self.title.set_font_description(Pango.FontDescription('Sans Bold 15'))
        self.add_tick_callback(self.tick)

    def tick(self, widget, clock):
        now = time.monotonic()
        elapsed = now - self.started
        start = 0
        for name, seconds in PHASES:
            if elapsed < start + seconds:
                if self.phase != name:
                    self.phase = name
                    self.phase_started = start
                    self.queue_draw()
                    report(event='phase', phase=name, elapsed_s=elapsed)
                break
            start += seconds
        else:
            self.app.quit()
            return False
        if now - self.last_report >= 1:
            report(event='draws', phase=self.phase, elapsed_s=elapsed,
                   frames=self.snapshots, fps=(self.snapshots - self.last_count) / (now - self.last_report),
                   scale=self.get_scale_factor(), width=self.get_width(), height=self.get_height())
            self.last_report = now
            self.last_count = self.snapshots
        # Static phases remain static, rather than forcing manufactured frames.
        if self.phase in ('scroll', 'motion') or self.snapshots == 0:
            self.queue_draw()
        return True

    @staticmethod
    def rect(x, y, width, height):
        return Graphene.Rect().init(x, y, width, height)

    @staticmethod
    def text(snapshot, layout, ink, x, y):
        snapshot.save()
        snapshot.translate(Graphene.Point().init(x, y))
        snapshot.append_layout(layout, ink)
        snapshot.restore()

    def do_snapshot(self, snapshot):
        self.snapshots += 1
        width, height = self.get_width(), self.get_height()
        snapshot.append_color(self.background, self.rect(0, 0, width, height))
        self.text(snapshot, self.title, self.ink, 28, 22)
        snapshot.push_clip(self.rect(0, 64, width, max(1, height - 124)))
        elapsed = time.monotonic() - self.started - self.phase_started
        offset = elapsed * 110 if self.phase == 'scroll' else 0
        first = int(offset // 28)
        for row in range(int(height / 28) + 2):
            layout, ink = self.lines[(first + row) % len(self.lines)]
            self.text(snapshot, layout, ink, 28, 72 + row * 28 - offset % 28)
        if self.phase == 'motion':
            x = (elapsed * 250) % max(1, width - 320)
            snapshot.append_color(self.blue, self.rect(x, 160, 300, 280))
        snapshot.pop()
        for n in range(16):
            gray = Gdk.RGBA()
            gray.red = gray.green = gray.blue = n / 15
            gray.alpha = 1
            snapshot.append_color(gray, self.rect(n * width / 16, height - 42, width / 16 + 1, 22))
        for x in range(30, min(width - 20, 510), 4):
            snapshot.append_color(self.ink, self.rect(x, height - 64, 1, 16))


def activate(app):
    monitors = Gdk.Display.get_default().get_monitors()
    tablet = next((m for m in monitors if (m.get_connector() or '').startswith('Meta-')), None)
    if tablet is None:
        report(event='error', message='No virtual tablet monitor')
        app.quit()
        return
    window = Gtk.ApplicationWindow(application=app, title='Extraspace sharpness test')
    window.set_child(Page(app))
    window.fullscreen_on_monitor(tablet)
    window.present()
    report(event='monitor', connector=tablet.get_connector(), scale=tablet.get_scale_factor())


application = Gtk.Application(application_id='io.github.sal0h.ExtraspaceSharpness',
                              flags=Gio.ApplicationFlags.NON_UNIQUE)
application.connect('activate', activate)
application.run(None)
