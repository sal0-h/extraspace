//! Native pairing UI. Keep QR credentials on screen only while pairing is open.
use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use xs_core::{Command, EngineHandle, WirelessEvent};

pub struct PairingUi {
    window: adw::ApplicationWindow,
    engine: EngineHandle,
    dialog: Rc<RefCell<Option<adw::Dialog>>>,
    devices_dialog: RefCell<Option<adw::Dialog>>,
}

impl PairingUi {
    pub fn new(window: &adw::ApplicationWindow, engine: &EngineHandle) -> Rc<Self> {
        Rc::new(Self {
            window: window.clone(),
            engine: engine.clone(),
            dialog: Rc::new(RefCell::new(None)),
            devices_dialog: RefCell::new(None),
        })
    }

    pub fn pair(&self) {
        if let Some(dialog) = self.dialog.borrow().as_ref() {
            dialog.present(Some(&self.window));
            return;
        }
        let Some(wireless) = self.engine.wireless.as_ref() else {
            self.message(
                "Wi-Fi unavailable",
                "Extraspace could not start its Wi-Fi service. Quit Extraspace and turn it on again.",
            );
            return;
        };
        let offer = match wireless.begin_pairing() {
            Ok(offer) => offer,
            Err(e) => {
                self.message("Could not start pairing", &e.to_string());
                return;
            }
        };
        let qr = match qrcode::QrCode::new(offer.qr.as_bytes()) {
            Ok(qr) => qr,
            Err(e) => {
                wireless.cancel_pairing();
                self.message("Could not create QR code", &e.to_string());
                return;
            }
        };
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            .margin_top(12)
            .margin_bottom(28)
            .margin_start(24)
            .margin_end(24)
            .build();
        let instructions = gtk::Label::builder().label("Open Extraspace on your tablet and tap “Pair computer”.\nScan this QR code, or enter the code below.")
            .wrap(true).justify(gtk::Justification::Center).build();
        content.append(&instructions);
        let drawing = gtk::DrawingArea::builder()
            .content_width(320)
            .content_height(320)
            .halign(gtk::Align::Center)
            .build();
        drawing.set_draw_func(move |_, context, width, height| {
            context.set_source_rgb(1.0, 1.0, 1.0);
            let _ = context.paint();
            let modules = qr.width();
            let scale = (f64::from(width.min(height)) / (modules + 8) as f64)
                .floor()
                .max(1.0);
            let x = (width as f64 - modules as f64 * scale) / 2.0;
            let y = (height as f64 - modules as f64 * scale) / 2.0;
            context.set_source_rgb(0.0, 0.0, 0.0);
            for row in 0..modules {
                for col in 0..modules {
                    if qr[(col, row)] == qrcode::Color::Dark {
                        context.rectangle(
                            x + col as f64 * scale,
                            y + row as f64 * scale,
                            scale,
                            scale,
                        );
                    }
                }
            }
            let _ = context.fill();
        });
        content.append(&drawing);
        let code = gtk::Label::builder()
            .label(&offer.code)
            .selectable(true)
            .build();
        code.add_css_class("monospace");
        code.add_css_class("title-3");
        content.append(&code);
        let expiry = gtk::Label::builder()
            .label("Valid for 5 minutes. Pair once; reconnect automatically.")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        expiry.add_css_class("dim-label");
        content.append(&expiry);
        let dialog = make_dialog("Pair tablet", &content);
        let wireless = std::sync::Arc::clone(wireless);
        let stored = Rc::downgrade(&self.dialog);
        dialog.connect_closed(move |_| {
            wireless.cancel_pairing();
            if let Some(stored) = stored.upgrade() {
                stored.borrow_mut().take();
            }
        });
        let weak = dialog.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_secs(300), move || {
            if let Some(dialog) = weak.upgrade() {
                drawing.set_sensitive(false);
                code.set_label("Code expired");
                expiry.set_label("Close this window and choose Pair tablet for a new code.");
                dialog.set_title("Pairing expired");
            }
        });
        dialog.present(Some(&self.window));
        *self.dialog.borrow_mut() = Some(dialog);
    }

    pub fn event(&self, event: &WirelessEvent) {
        if matches!(event, WirelessEvent::Paired(_)) {
            let dialog = self.dialog.borrow_mut().take();
            if let Some(dialog) = dialog {
                dialog.close();
            }
        }
        if matches!(event, WirelessEvent::DevicesChanged) {
            let dialog = self.devices_dialog.borrow_mut().take();
            if let Some(dialog) = dialog {
                dialog.close();
            }
        }
    }

    pub fn devices(&self) {
        let peers = self
            .engine
            .wireless
            .as_ref()
            .map(|w| w.paired_devices())
            .unwrap_or_default();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let description = gtk::Label::builder().label("Saved tablets can connect while Extraspace is turned on. Forget a tablet to revoke its access.")
            .wrap(true).xalign(0.0).build();
        content.append(&description);
        let group = adw::PreferencesGroup::new();
        for peer in peers {
            let row = adw::ActionRow::builder()
                .title(&peer.name)
                .use_markup(false)
                .subtitle("Trusted tablet")
                .build();
            let forget = gtk::Button::builder()
                .label("Forget")
                .valign(gtk::Align::Center)
                .build();
            forget.add_css_class("destructive-action");
            let engine = self.engine.clone();
            forget.connect_clicked(move |_| {
                engine.send(Command::ForgetDevice(peer.id.clone()));
            });
            row.add_suffix(&forget);
            group.add(&row);
        }
        content.append(&group);
        let dialog = make_dialog("Paired devices", &content);
        dialog.present(Some(&self.window));
        *self.devices_dialog.borrow_mut() = Some(dialog);
    }

    fn message(&self, title: &str, body: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(title)
            .body(body)
            .build();
        dialog.add_response("close", "Close");
        dialog.present(Some(&self.window));
    }
}

fn make_dialog(title: &str, content: &impl IsA<gtk::Widget>) -> adw::Dialog {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(content)
        .build();
    toolbar.set_content(Some(&scroll));
    adw::Dialog::builder()
        .title(title)
        .content_width(440)
        .content_height(600)
        .child(&toolbar)
        .build()
}
