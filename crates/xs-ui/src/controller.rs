//! Application lifetime and controls, independent of the optional settings window.
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use xs_core::{Command, DisplayMode, EngineHandle, Event, State, Stats};

use crate::config::{scale_index, Config, SCALE_OPTIONS};
use crate::window::WindowUi;

pub struct Controller {
    app: glib::WeakRef<adw::Application>,
    pub config: Rc<RefCell<Config>>,
    pub engine: EngineHandle,
    enabled: Cell<bool>,
    stopping: Cell<bool>,
    state: RefCell<State>,
    stats: RefCell<Stats>,
    window: RefCell<Option<Rc<WindowUi>>>,
    session_state: gio::SimpleAction,
    display_enabled: gio::SimpleAction,
    window_visible: gio::SimpleAction,
    _hold: gio::ApplicationHoldGuard,
}

impl Controller {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let config = Rc::new(RefCell::new(Config::load()));
        let engine = xs_core::spawn(crate::session_config(&config.borrow()));
        let session_state = read_only_action(app, "session-state", &"\"Idle\"".to_variant());
        let display_enabled = read_only_action(app, "display-enabled", &false.to_variant());
        let window_visible = read_only_action(app, "window-visible", &false.to_variant());
        let this = Rc::new(Self {
            app: app.downgrade(),
            config,
            engine,
            enabled: Cell::new(false),
            stopping: Cell::new(false),
            state: RefCell::new(State::Idle),
            stats: RefCell::new(Stats::default()),
            window: RefCell::new(None),
            session_state,
            display_enabled,
            window_visible,
            _hold: app.hold(),
        });
        this.action("quit", None, |this, _| this.quit());
        this.action("show-settings", None, |this, _| this.show_settings());
        this.action("hide-settings", None, |this, _| this.close_settings());
        this.action(
            "set-display",
            Some(&bool::static_variant_type()),
            |this, value| match value.and_then(|v| v.get::<bool>()) {
                Some(true) => this.start(),
                Some(false) => this.quit(),
                None => {}
            },
        );
        this.action(
            "set-scale",
            Some(&f64::static_variant_type()),
            |this, value| {
                if let Some(scale) = value.and_then(|v| v.get::<f64>()).filter(|v| v.is_finite()) {
                    this.set_scale(scale);
                }
            },
        );
        this.action(
            "set-mode",
            Some(&String::static_variant_type()),
            |this, value| match value.and_then(|v| v.str()) {
                Some("extend") => this.set_mode(DisplayMode::Extend),
                Some("mirror") => this.set_mode(DisplayMode::Mirror),
                _ => {}
            },
        );
        this.action("pair-tablet", None, |this, _| this.pair_tablet());
        this.action(
            "set-native-resolution",
            Some(&bool::static_variant_type()),
            |this, value| {
                if let Some(native) = value.and_then(|v| v.get::<bool>()) {
                    this.set_native_resolution(native);
                }
            },
        );
        this.action("paired-devices", None, |this, _| {
            this.show_settings();
            if let Some(window) = this.window.borrow().as_ref() {
                window.devices();
            }
        });
        this.action(
            "save-video-diagnostic",
            Some(&String::static_variant_type()),
            |this, value| {
                if let Some(directory) = value.and_then(|v| v.str()) {
                    this.engine
                        .send(Command::SaveVideoDiagnostic(PathBuf::from(directory)));
                }
            },
        );
        app.set_accels_for_action("app.quit", &["<Primary>q"]);
        this.listen();
        this
    }

    fn action(
        self: &Rc<Self>,
        name: &str,
        parameter: Option<&glib::VariantTy>,
        callback: impl Fn(&Rc<Self>, Option<&glib::Variant>) + 'static,
    ) {
        let action = gio::SimpleAction::new(name, parameter);
        let weak = Rc::downgrade(self);
        action.connect_activate(move |_, value| {
            if let Some(this) = weak.upgrade() {
                callback(&this, value);
            }
        });
        self.app.upgrade().unwrap().add_action(&action);
    }

    pub fn start(&self) {
        if self.stopping.get() {
            return;
        }
        self.enabled.set(true);
        self.display_enabled.set_state(&true.to_variant());
        self.render();
        self.engine.send(Command::Connect);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.get()
    }

    pub fn quit(self: &Rc<Self>) {
        if self.stopping.replace(true) {
            return;
        }
        self.enabled.set(false);
        self.display_enabled.set_state(&false.to_variant());
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            this.engine.shutdown().await;
            if let Some(app) = this.app.upgrade() {
                app.quit();
            }
        });
    }

    pub fn show_settings(self: &Rc<Self>) {
        if self.stopping.get() {
            return;
        }
        if self.window.borrow().is_none() {
            let app = self.app.upgrade().unwrap();
            let window = crate::window::build(&app, Rc::clone(self));
            self.window.replace(Some(window));
        }
        self.render();
        self.window.borrow().as_ref().unwrap().present();
        self.window_visible.set_state(&true.to_variant());
    }

    pub fn close_settings(self: &Rc<Self>) {
        if let Some(window) = self.window.borrow().as_ref() {
            window.hide();
        }
        self.window_visible.set_state(&false.to_variant());
        if !self.enabled.get() {
            self.quit();
        }
    }

    pub fn pair_tablet(self: &Rc<Self>) {
        self.start();
        self.show_settings();
        if let Some(window) = self.window.borrow().as_ref() {
            window.pair();
        }
    }

    pub fn set_scale(&self, scale: f64) {
        let scale = SCALE_OPTIONS[scale_index(scale) as usize];
        if self.config.borrow().scale == scale {
            return;
        }
        self.config.borrow_mut().scale = scale;
        self.config.borrow().save();
        self.engine.send(Command::SetScale(scale));
        self.render();
    }

    pub fn set_mode(&self, mode: DisplayMode) {
        if self.config.borrow().display_mode() == mode {
            return;
        }
        self.config.borrow_mut().mode = if mode == DisplayMode::Mirror {
            "mirror"
        } else {
            "extend"
        }
        .into();
        self.config.borrow().save();
        self.engine.send(Command::SetMode(mode));
        self.render();
    }

    pub fn set_native_resolution(&self, native: bool) {
        if self.config.borrow().native_resolution == native {
            return;
        }
        self.config.borrow_mut().native_resolution = native;
        self.config.borrow().save();
        self.engine.send(Command::SetNativeResolution(native));
        self.render();
    }

    pub fn set_camera(&self, enabled: bool) {
        if self.config.borrow().camera_enabled == enabled {
            return;
        }
        self.config.borrow_mut().camera_enabled = enabled;
        self.config.borrow().save();
        self.engine.send(Command::SetCamera {
            enabled,
            camera_id: self.config.borrow().camera_id.clone(),
        });
    }

    fn render(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            window.render(
                &self.state.borrow(),
                &self.stats.borrow(),
                self.enabled.get(),
                &self.config.borrow(),
            );
        }
    }

    fn listen(self: &Rc<Self>) {
        let mut events = self.engine.subscribe();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let Some(this) = weak.upgrade() else {
                    break;
                };
                match &event {
                    Event::State(state) => {
                        this.state.replace(state.clone());
                        if let Ok(json) = serde_json::to_string(state) {
                            this.session_state.set_state(&json.to_variant());
                        }
                    }
                    Event::Stats(stats) => {
                        this.stats.replace(stats.clone());
                    }
                    _ => {}
                }
                this.render();
                if let Some(window) = this.window.borrow().as_ref() {
                    window.event(&event);
                };
            }
        });
    }
}

fn read_only_action(
    app: &adw::Application,
    name: &str,
    state: &glib::Variant,
) -> gio::SimpleAction {
    let action = gio::SimpleAction::new_stateful(name, None, state);
    action.set_enabled(false);
    app.add_action(&action);
    action
}
