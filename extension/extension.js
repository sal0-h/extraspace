import Gio from 'gi://Gio';
import GioUnix from 'gi://GioUnix';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as QuickSettings from 'resource:///org/gnome/shell/ui/quickSettings.js';

import {describeSession} from './state.js';

const APP = 'io.github.tymonoman.Extraspace';
const OBJECT_PATH = '/io/github/tymonoman/Extraspace';

const ExtraspaceToggle = GObject.registerClass(
class ExtraspaceToggle extends QuickSettings.QuickMenuToggle {
    constructor() {
        super({
            title: 'Extraspace',
            subtitle: 'Off',
            iconName: 'video-display-symbolic',
            toggleMode: false,
        });
        this.menu.setHeader('video-display-symbolic', 'Extraspace', 'Tablet display');
        this._status = new PopupMenu.PopupMenuItem('Display is off', {reactive: false});
        this.menu.addMenuItem(this._status);
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this.menu.addAction('Settings', () => this._open('Settings'));
        this.menu.addAction('Pair tablet', () => this._pair());
        this.connect('clicked', () => this._toggle());

        this._signals = [];
        this._watch = Gio.bus_watch_name(Gio.BusType.SESSION, APP,
            Gio.BusNameWatcherFlags.NONE,
            connection => this._attach(connection),
            () => this._detach());
    }

    _attach(connection) {
        this._disconnectSignals();
        this._actions = Gio.DBusActionGroup.get(connection, APP, OBJECT_PATH);
        for (const signal of ['action-added', 'action-state-changed'])
            this._signals.push(this._actions.connect(signal, () => this._sync()));
        this._sync();
    }

    _disconnectSignals() {
        for (const id of this._signals)
            this._actions.disconnect(id);
        this._signals = [];
        this._actions = null;
    }

    _detach() {
        this._disconnectSignals();
        // A name-vanished notification is also delivered before a first launch.
        if (this._pending === 'on')
            return;
        this._finishTransition();
        this._sync();
    }

    _sync() {
        const enabled = this._actions?.get_action_state('display-enabled')?.get_boolean() ?? false;
        if (this._pending === 'on' && enabled)
            this._finishTransition();
        const json = this._actions?.get_action_state('session-state')?.get_string()[0];
        const status = describeSession(enabled, json, this._pending);
        this.checked = status.checked;
        this.subtitle = status.subtitle;
        this._status.label.text = status.detail;
        this.reactive = !this._pending;
    }

    _beginTransition(direction) {
        this._pending = direction;
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 20, () => {
            this._timer = 0;
            this._pending = null;
            this._sync();
            Main.notifyError('Extraspace', 'The display did not respond. Open Settings to check the connection.');
            return GLib.SOURCE_REMOVE;
        });
        this._sync();
    }

    _finishTransition() {
        if (this._timer)
            GLib.source_remove(this._timer);
        this._timer = 0;
        this._pending = null;
    }

    _toggle() {
        if (this._pending)
            return;
        const enabled = this._actions?.get_action_state('display-enabled')?.get_boolean() ?? false;
        this._beginTransition(enabled ? 'off' : 'on');
        try {
            if (this._actions?.has_action('set-display')) {
                this._actions.activate_action('set-display', new GLib.Variant('b', !enabled));
            } else {
                this._launch('Background');
            }
        } catch (error) {
            this._finishTransition();
            this._sync();
            Main.notifyError('Extraspace', error.message);
        }
    }

    _launch(action) {
        const app = GioUnix.DesktopAppInfo.new(`${APP}.desktop`);
        if (!app || !app.list_actions().includes(action))
            throw new Error('Reinstall Extraspace to enable its Quick Settings controls.');
        app.launch_action(action, global.create_app_launch_context(0, -1));
        Main.panel.statusArea.quickSettings.menu.close();
    }

    _open(action) {
        try {
            this._launch(action);
        } catch (error) {
            Main.notifyError('Extraspace', error.message);
        }
    }

    _pair() {
        if (this._actions?.has_action('pair-tablet')) {
            this._actions.activate_action('pair-tablet', null);
            Main.panel.statusArea.quickSettings.menu.close();
        } else {
            this._open('Pair');
        }
    }

    destroy() {
        Gio.bus_unwatch_name(this._watch);
        this._disconnectSignals();
        this._finishTransition();
        super.destroy();
    }
});

export default class ExtraspaceExtension extends Extension {
    enable() {
        this._indicator = new QuickSettings.SystemIndicator();
        this._indicator.quickSettingsItems.push(new ExtraspaceToggle());
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._indicator);
    }

    disable() {
        for (const item of this._indicator.quickSettingsItems)
            item.destroy();
        this._indicator.destroy();
        this._indicator = null;
    }
}
