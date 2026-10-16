//! The main window.
//!
//! Two faces, switched by a [`gtk::Stack`]:
//!
//! * a full-bleed [`adw::StatusPage`] whenever there is nothing to control --
//!   no tablet, not authorised, connecting, or failed;
//! * an [`adw::PreferencesPage`] once a session is live.
//!
//! Failure states get first-class treatment rather than a generic error dialog.
//! The two most likely first-run problems -- no tablet, and USB debugging not
//! authorised -- are entirely fixable by the user, so each gets its own page
//! saying exactly which buttons to press.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use xs_core::{DisplayMode, Event, State, Stats};

use crate::config::{scale_index, Config, SCALE_OPTIONS};
use crate::controller::Controller;

pub const APP_ID: &str = "io.github.tymonoman.Extraspace";

struct Widgets {
    stack: gtk::Stack,
    status: adw::StatusPage,
    status_button: gtk::Button,
    pair_button: gtk::Button,
    spinner: gtk::Spinner,
    window_title: adw::WindowTitle,
    toasts: adw::ToastOverlay,

    display_switch: adw::SwitchRow,
    display_switch_handler: RefCell<Option<glib::SignalHandlerId>>,
    mode_row: adw::ComboRow,
    scale_row: adw::ComboRow,
    camera_switch: adw::SwitchRow,

    stat_resolution: adw::ActionRow,
    stat_connection: adw::ActionRow,
    stat_bitrate: adw::ActionRow,
    stat_fps: adw::ActionRow,
    stat_latency: adw::ActionRow,
    stat_encoder: adw::ActionRow,
}

pub struct WindowUi {
    window: adw::ApplicationWindow,
    widgets: Rc<Widgets>,
    pairing: Rc<crate::pairing::PairingUi>,
}

impl WindowUi {
    pub fn present(&self) {
        self.window.present();
    }
    pub fn hide(&self) {
        self.window.set_visible(false);
    }
    pub fn pair(&self) {
        self.pairing.pair();
    }
    pub fn devices(&self) {
        self.pairing.devices();
    }

    pub fn render(&self, state: &State, stats: &Stats, enabled: bool, config: &Config) {
        apply_state(&self.widgets, state, enabled);
        apply_stats(&self.widgets, stats);
        self.widgets
            .scale_row
            .set_selected(scale_index(config.scale));
        self.widgets
            .mode_row
            .set_selected(if config.display_mode() == DisplayMode::Mirror {
                1
            } else {
                0
            });
        self.widgets.camera_switch.set_active(config.camera_enabled);
        update_scale_subtitle(&self.widgets, config.scale);
    }

    pub fn event(&self, event: &Event) {
        match event {
            Event::Warning(message) => {
                self.widgets.toasts.add_toast(adw::Toast::new(message));
            }
            Event::Wireless(event) => {
                self.pairing.event(event);
                if let xs_core::WirelessEvent::Paired(peer) = event {
                    self.widgets
                        .toasts
                        .add_toast(adw::Toast::new(&format!("{} paired", peer.name)));
                }
            }
            _ => {}
        }
    }
}

pub fn build(app: &adw::Application, controller: Rc<Controller>) -> Rc<WindowUi> {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Extraspace")
        .default_width(560)
        .default_height(720)
        .width_request(360)
        .height_request(400)
        .build();

    let window_title = adw::WindowTitle::new("Extraspace", "Not connected");

    let menu = gio_menu();
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Main Menu")
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&window_title)
        .build();
    header.pack_end(&menu_button);

    let (page, widgets) = build_content(window_title.clone());

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&widgets.toasts));
    widgets.toasts.set_child(Some(&page));
    window.set_content(Some(&toolbar));

    let widgets = Rc::new(widgets);
    let pairing = crate::pairing::PairingUi::new(&window, &controller.engine);
    let pairing_button = Rc::clone(&controller);
    widgets.pair_button.connect_clicked(move |_| {
        pairing_button.pair_tablet();
    });
    wire_actions(app, &window);
    wire_controls(&widgets, &controller);
    // Closing settings leaves the display running. Off and Quit end the engine.
    let on_close = Rc::clone(&controller);
    window.connect_close_request(move |_| {
        on_close.close_settings();
        glib::Propagation::Stop
    });
    Rc::new(WindowUi {
        window,
        widgets,
        pairing,
    })
}

fn build_content(window_title: adw::WindowTitle) -> (gtk::Widget, Widgets) {
    let toasts = adw::ToastOverlay::new();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();

    // ---- the "nothing to control yet" face ----
    let status_button = gtk::Button::builder()
        .label("Check Again")
        .halign(gtk::Align::Center)
        .build();
    status_button.add_css_class("pill");
    status_button.add_css_class("suggested-action");

    let spinner = gtk::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .build();

    let status_extra = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    status_extra.append(&spinner);
    status_extra.append(&status_button);
    let pair_button = gtk::Button::with_label("Pair tablet over Wi-Fi");
    pair_button.add_css_class("pill");
    status_extra.append(&pair_button);

    let status = adw::StatusPage::builder()
        .icon_name("computer-symbolic")
        .title("Looking for your tablet")
        .child(&status_extra)
        .build();
    stack.add_named(&status, Some("status"));

    // ---- the live control face ----
    let prefs = adw::PreferencesPage::new();

    let display_group = adw::PreferencesGroup::builder()
        .title("Display")
        .description("Use the tablet as an extra monitor")
        .build();

    let display_switch = adw::SwitchRow::builder()
        .title("Extra Display")
        .subtitle("Stream your desktop to the tablet")
        .build();
    display_group.add(&display_switch);

    let mode_model = gtk::StringList::new(&["Extend", "Mirror"]);
    let mode_row = adw::ComboRow::builder()
        .title("Mode")
        .subtitle("Extend adds a new monitor; Mirror copies an existing one")
        .model(&mode_model)
        .build();
    display_group.add(&mode_row);

    let scale_labels: Vec<String> = SCALE_OPTIONS.iter().map(|s| format!("{s}×")).collect();
    let scale_refs: Vec<&str> = scale_labels.iter().map(String::as_str).collect();
    let scale_row = adw::ComboRow::builder()
        .title("Scale")
        .subtitle("Desktop size on the tablet")
        .model(&gtk::StringList::new(&scale_refs))
        .build();
    display_group.add(&scale_row);
    prefs.add(&display_group);

    let camera_group = adw::PreferencesGroup::builder()
        .title("Camera")
        .description("Expose the tablet camera to Linux as a normal webcam")
        .build();
    let camera_switch = adw::SwitchRow::builder()
        .title("Tablet Camera")
        .subtitle("Appears as “Extraspace Tablet Camera”")
        .build();
    camera_group.add(&camera_switch);
    prefs.add(&camera_group);

    let stats_group = adw::PreferencesGroup::builder().title("Statistics").build();
    let stat_connection = stat_row("Connection", "network-transmit-receive-symbolic");
    let stat_resolution = stat_row("Resolution", "video-display-symbolic");
    let stat_bitrate = stat_row("Bitrate", "network-transmit-symbolic");
    let stat_fps = stat_row("Frame Rate", "preferences-system-time-symbolic");
    let stat_latency = stat_row("Connection round trip", "network-wireless-symbolic");
    let stat_encoder = stat_row("Encoder", "applications-multimedia-symbolic");
    for row in [
        &stat_connection,
        &stat_resolution,
        &stat_bitrate,
        &stat_fps,
        &stat_latency,
        &stat_encoder,
    ] {
        stats_group.add(row);
    }
    prefs.add(&stats_group);

    stack.add_named(&prefs, Some("running"));
    stack.set_visible_child_name("status");

    let widgets = Widgets {
        stack: stack.clone(),
        status,
        status_button,
        pair_button,
        spinner,
        window_title,
        toasts,
        display_switch,
        display_switch_handler: RefCell::new(None),
        mode_row,
        scale_row,
        camera_switch,
        stat_resolution,
        stat_connection,
        stat_bitrate,
        stat_fps,
        stat_latency,
        stat_encoder,
    };
    (stack.upcast(), widgets)
}

fn stat_row(title: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle("—").build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.set_subtitle_selectable(true);
    row
}

fn gio_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("_Pair tablet"), Some("app.pair-tablet"));
    menu.append(Some("Paired _devices"), Some("app.paired-devices"));
    menu.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    menu.append(Some("_About Extraspace"), Some("app.about"));
    menu.append(Some("_Quit"), Some("app.quit"));
    menu
}

fn wire_actions(app: &adw::Application, window: &adw::ApplicationWindow) {
    let about = gtk::gio::SimpleAction::new("about", None);
    let parent = window.clone();
    about.connect_activate(move |_, _| {
        adw::AboutDialog::builder()
            .application_name("Extraspace")
            .application_icon(APP_ID)
            .developer_name("Tymonoman")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/Tymonoman/extraspace")
            .issue_url("https://github.com/Tymonoman/extraspace/issues")
            .license_type(gtk::License::Gpl30)
            .comments("Use an Android tablet as an extra display and webcam, over USB.")
            .build()
            .present(Some(&parent));
    });
    app.add_action(&about);
}

fn wire_controls(widgets: &Rc<Widgets>, controller: &Rc<Controller>) {
    // Restore settings before attaching notifications.
    {
        let c = controller.config.borrow();
        widgets.scale_row.set_selected(scale_index(c.scale));
        widgets
            .mode_row
            .set_selected(if c.display_mode() == DisplayMode::Mirror {
                1
            } else {
                0
            });
        widgets.camera_switch.set_active(c.camera_enabled);
        update_scale_subtitle(widgets, c.scale);
    }
    let display = Rc::clone(controller);
    let handler = widgets.display_switch.connect_active_notify(move |row| {
        if row.is_active() {
            display.start();
        } else {
            display.quit();
        }
    });
    widgets.display_switch_handler.replace(Some(handler));
    let scale = Rc::clone(controller);
    widgets.scale_row.connect_selected_notify(move |row| {
        if let Some(&value) = SCALE_OPTIONS.get(row.selected() as usize) {
            scale.set_scale(value);
        }
    });
    let mode = Rc::clone(controller);
    widgets.mode_row.connect_selected_notify(move |row| {
        mode.set_mode(if row.selected() == 1 {
            DisplayMode::Mirror
        } else {
            DisplayMode::Extend
        });
    });
    let camera = Rc::clone(controller);
    widgets
        .camera_switch
        .connect_active_notify(move |row| camera.set_camera(row.is_active()));
    let connect = Rc::clone(controller);
    widgets
        .status_button
        .connect_clicked(move |_| connect.start());
}

fn update_scale_subtitle(widgets: &Rc<Widgets>, scale: f64) {
    let description = if xs_core::modes_enabled() {
        "GNOME scale at panel resolution"
    } else {
        "smaller video frame, enlarged on the tablet"
    };
    widgets
        .scale_row
        .set_subtitle(&format!("{scale}× — {description}"));
}

fn apply_state(widgets: &Rc<Widgets>, state: &State, enabled: bool) {
    // Automatic Wi-Fi connections and the status-page Connect button bypass
    // the switch. Render the engine's state without echoing a new command.
    {
        let handler = widgets.display_switch_handler.borrow();
        if let Some(handler) = handler.as_ref() {
            widgets.display_switch.block_signal(handler);
        }
        widgets.display_switch.set_active(enabled);
        if let Some(handler) = handler.as_ref() {
            widgets.display_switch.unblock_signal(handler);
        }
    }
    let show_status = |icon: &str, title: &str, description: &str, button: Option<&str>| {
        widgets.status.set_icon_name(Some(icon));
        widgets.status.set_title(title);
        widgets.status.set_description(Some(description));
        widgets.spinner.set_visible(false);
        widgets.spinner.stop();
        match button {
            Some(label) => {
                widgets.status_button.set_label(label);
                widgets.status_button.set_visible(true);
            }
            None => widgets.status_button.set_visible(false),
        }
        widgets.stack.set_visible_child_name("status");
    };

    match state {
        State::Idle => {
            widgets.window_title.set_subtitle("Not connected");
            show_status(
                "video-display-symbolic",
                "Ready",
                "Choose Connect to start streaming to your tablet.",
                Some("Connect"),
            );
        }

        State::NoTablet => {
            widgets.window_title.set_subtitle("No tablet");
            show_status(
                "phone-disconnected-symbolic",
                "Ready for your tablet",
                "Open Extraspace on your paired tablet on the same Wi-Fi.\n\n\
                 First time? Choose “Pair tablet over Wi-Fi” below.\n\
                 You can also connect your tablet with a USB cable.",
                Some("Check USB connection"),
            );
        }

        State::Unauthorized { device } => {
            widgets.window_title.set_subtitle("Not authorised");
            show_status(
                "dialog-password-symbolic",
                "Allow USB debugging",
                &format!(
                    "{device} is connected but has not granted permission yet.\n\n\
                     Unlock the tablet and tap “Allow” on the USB debugging prompt. \
                     Tick “Always allow from this computer” so you are not asked again.",
                ),
                Some("Try Again"),
            );
        }

        State::Connecting { step } => {
            widgets.window_title.set_subtitle("Connecting…");
            widgets
                .status
                .set_icon_name(Some("content-loading-symbolic"));
            widgets.status.set_title("Connecting");
            widgets.status.set_description(Some(step));
            widgets.status_button.set_visible(false);
            widgets.spinner.set_visible(true);
            widgets.spinner.start();
            widgets.stack.set_visible_child_name("status");
        }

        State::WaitingForTablet { device, connection } => {
            widgets
                .window_title
                .set_subtitle(&format!("Waiting · {connection}"));
            let description = if connection == "Wi-Fi" {
                format!("Open Extraspace on {device} on the same Wi-Fi. It will reconnect automatically.")
            } else {
                format!("Open Extraspace on {device} to resume over {connection}. If the cable is unplugged, reconnect it.")
            };
            show_status(
                "phone-disconnected-symbolic",
                "Waiting for your tablet",
                &description,
                None,
            );
        }

        State::Streaming {
            device,
            connection,
            width,
            height,
            encoder,
        } => {
            widgets.window_title.set_subtitle(device);
            widgets.stat_connection.set_subtitle(connection);
            widgets.spinner.stop();
            widgets
                .stat_resolution
                .set_subtitle(&format!("{width} × {height} streamed"));
            widgets.stat_encoder.set_subtitle(encoder);
            widgets.stack.set_visible_child_name("running");
        }

        State::Failed { message } => {
            widgets.window_title.set_subtitle("Error");
            show_status(
                "dialog-warning-symbolic",
                "Something went wrong",
                message,
                Some("Try Again"),
            );
        }
    }
}

fn apply_stats(widgets: &Rc<Widgets>, stats: &Stats) {
    widgets
        .stat_bitrate
        .set_subtitle(&xs_core::format_bitrate(stats.bitrate_kbps));
    widgets
        .stat_fps
        .set_subtitle(&format!("{:.0} fps", stats.fps));
    // Bound to a local: a `format!` temporary inside the call would not outlive it.
    let latency = if stats.latency_ms > 0.0 {
        format!("{:.0} ms", stats.latency_ms)
    } else {
        "measuring…".to_owned()
    };
    widgets.stat_latency.set_subtitle(&latency);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_connections_sync_the_display_switch_without_sending_commands() {
        adw::init().expect("run GTK tests under xvfb-run when no display is available");
        let (_page, widgets) = build_content(adw::WindowTitle::new("Extraspace", ""));
        let widgets = Rc::new(widgets);
        let commands = Rc::new(RefCell::new(Vec::new()));
        let sent = Rc::clone(&commands);
        let handler = widgets.display_switch.connect_active_notify(move |row| {
            sent.borrow_mut().push(row.is_active());
        });
        widgets.display_switch_handler.replace(Some(handler));

        let streaming = State::Streaming {
            device: "Tablet".into(),
            connection: "Wi-Fi".into(),
            width: 1536,
            height: 960,
            encoder: "test".into(),
        };
        apply_state(&widgets, &streaming, true);
        assert!(widgets.display_switch.is_active());
        assert_eq!(
            widgets.stack.visible_child_name().as_deref(),
            Some("running")
        );
        assert!(commands.borrow().is_empty());

        // Off must still send the user's disconnect request. The engine can
        // then report Idle, followed by a later automatic Wi-Fi connection.
        widgets.display_switch.set_active(false);
        assert_eq!(*commands.borrow(), vec![false]);
        for state in [
            State::Idle,
            State::NoTablet,
            State::Failed {
                message: "Connection lost".into(),
            },
            streaming,
            State::WaitingForTablet {
                device: "Tablet".into(),
                connection: "Wi-Fi".into(),
            },
            State::Connecting {
                step: "Reconnecting".into(),
            },
        ] {
            let enabled = matches!(
                state,
                State::Streaming { .. } | State::WaitingForTablet { .. } | State::Connecting { .. }
            );
            apply_state(&widgets, &state, enabled);
            assert_eq!(widgets.display_switch.is_active(), enabled, "{state:?}");
            assert_eq!(*commands.borrow(), vec![false]);
        }
        apply_state(&widgets, &State::Idle, false);
        widgets.display_switch.set_active(true);
        assert_eq!(*commands.borrow(), vec![false, true]);
    }
}
