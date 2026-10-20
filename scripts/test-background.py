#!/usr/bin/env python3
"""Run under dbus-run-session and Xvfb; exercises the actual GApplication service."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time

from gi.repository import Gio, GLib

BINARY = str(Path(sys.argv[1]).resolve())
DEST = 'io.github.tymonoman.Extraspace'
OBJECT = '/io/github/tymonoman/Extraspace'
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

def wait(check, message, seconds=12):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except GLib.Error:
            # Name ownership can precede action export during startup.
            pass
        time.sleep(.1)
    raise AssertionError(message)

def owner():
    return bus.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
                         'org.freedesktop.DBus', 'NameHasOwner',
                         GLib.Variant('(s)', (DEST,)), None,
                         Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]

def state(name):
    return bus.call_sync(DEST, OBJECT, 'org.gtk.Actions', 'Describe',
                         GLib.Variant('(s)', (name,)), None,
                         Gio.DBusCallFlags.NONE, 3000, None).unpack()[0][2][0]

def action(name, value=None):
    args = [] if value is None else [value]
    bus.call_sync(DEST, OBJECT, 'org.gtk.Actions', 'Activate',
                  GLib.Variant('(sava{sv})', (name, args, {})), None,
                  Gio.DBusCallFlags.NONE, 3000, None)

assert not owner(), 'Use a private dbus-run-session, not the real desktop bus'
with tempfile.TemporaryDirectory(prefix='extraspace-lifecycle-') as temporary:
    root = Path(temporary)
    adb_dir = root / 'bin'
    adb_dir.mkdir()
    adb = adb_dir / 'adb'
    adb_log = root / 'adb.log'
    adb.write_text(f'''#!/bin/sh
printf '%s\\n' "$*" >> {shlex.quote(str(adb_log))}
if [ "$*" = "devices -l" ]; then printf 'List of devices attached\\n'; else exit 1; fi
''')
    adb.chmod(0o755)
    env = dict(os.environ, XDG_CONFIG_HOME=str(root / 'config'), PATH=str(adb_dir))
    processes = []
    def launch(*args):
        p = subprocess.Popen([BINARY, *args], env=env)
        processes.append(p)
        wait(owner, 'Application did not register')
        wait(lambda: state('window-visible') == ('--background' not in args), 'Wrong initial window visibility')
        return p
    def stopped(p):
        wait(lambda: not owner(), 'Off left the application running')
        assert p.wait(timeout=5) == 0
    try:
        p = launch('--settings')
        assert not state('display-enabled'), 'Opening settings started the display'
        action('hide-settings')
        stopped(p)
        print('Settings while off closes completely', flush=True)

        p = launch('--background')
        wait(lambda: state('display-enabled'), 'Background launch did not enable display')
        wait(lambda: json.loads(state('session-state')) == 'NoTablet', 'Expected waiting state without hardware')
        for _ in range(2):
            action('show-settings')
            wait(lambda: state('window-visible'), 'Settings did not open')
            action('hide-settings')
            wait(lambda: not state('window-visible'), 'Settings did not close')
            assert state('display-enabled'), 'Closing settings stopped the display'
            assert p.poll() is None
        print('Background launch has no window; settings can open and close without stopping', flush=True)

        action('set-scale', GLib.Variant('d', 1.25))
        action('set-mode', GLib.Variant('s', 'mirror'))
        action('set-native-resolution', GLib.Variant('b', False))
        config_file = root / 'config' / 'extraspace' / 'config.json'
        wait(lambda: config_file.exists(), 'Settings did not persist')
        saved = json.loads(config_file.read_text())
        assert saved['scale'] == 1.25 and saved['mode'] == 'mirror'
        assert not saved['native_resolution']
        action('set-native-resolution', GLib.Variant('b', True))
        wait(lambda: json.loads(config_file.read_text())['native_resolution'], 'Sharper preference did not persist')
        assert not state('window-visible'), 'Changing settings created a window'
        action('set-display', GLib.Variant('b', False))
        stopped(p)
        print('Controls work without a window; Off exits fully', flush=True)

        p = launch('--background')
        wait(lambda: state('display-enabled'), 'Second start did not enable display')
        wait(lambda: json.loads(state('session-state')) == 'NoTablet', 'Expected waiting state')
        commands = adb_log.read_text()
        # Reopening from the app grid should only show settings, including
        # while an enabled display is waiting for its tablet.
        remote = subprocess.run([BINARY], env=env, timeout=8)
        assert remote.returncode == 0
        wait(lambda: state('window-visible'), 'Second launch did not open existing settings')
        assert state('display-enabled') and p.poll() is None
        time.sleep(.3)
        assert adb_log.read_text() == commands, 'Opening settings retried the tablet connection'
        action('hide-settings')
        action('quit')
        stopped(p)
        print('Repeated launches use one instance and preserve display state', flush=True)

        p = launch('--pair')
        wait(lambda: state('display-enabled'), 'Pairing did not enable display')
        action('quit')
        stopped(p)
        print('Pair action opens the existing pairing UI and quits cleanly', flush=True)
    finally:
        if owner():
            action('quit')
        for p in processes:
            try:
                p.wait(timeout=8)
            except subprocess.TimeoutExpired:
                p.kill()
                p.wait()
