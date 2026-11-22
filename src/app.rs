//! egui user interface and controller wiring.

use eframe::egui;
use std::time::Duration;

use crate::proto::{InputEvent, MonitorInfo, SessionInfo};
use crate::session::{
    Decision, HostEvent, HostSession, ViewerEvent, ViewerSession, ViewerStats, DEFAULT_PORT,
};
use crate::util;

#[derive(Clone, Debug)]
pub enum StartMode {
    Gui,
    Host { port: u16 },
    Viewer { address: String, code: Option<String> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Host,
    Viewer,
}

struct Incoming {
    peer: String,
    client_name: String,
    session_id: u64,
}

struct HostActive {
    peer: String,
    session_id: u64,
    fps: f32,
    kbps: f32,
    encode_ms: f32,
    queue: usize,
    bitrate_kbps: u32,
}

struct HostUi {
    port: String,
    session: Option<HostSession>,
    code: String,
    addresses: Vec<String>,
    incoming: Option<Incoming>,
    active: Option<HostActive>,
    warning: Option<String>,
    error: Option<String>,
}

impl Default for HostUi {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT.to_string(),
            session: None,
            code: String::new(),
            addresses: Vec::new(),
            incoming: None,
            active: None,
            warning: None,
            error: None,
        }
    }
}

impl HostUi {
    fn start(&mut self) {
        let port: u16 = self
            .port
            .trim()
            .parse()
            .ok()
            .filter(|p| *p != 0)
            .unwrap_or(DEFAULT_PORT);
        self.error = None;
        self.warning = None;
        self.active = None;
        self.incoming = None;
        self.session = Some(HostSession::start(port));
    }

    fn stop(&mut self) {
        if let Some(s) = &self.session {
            s.stop_listener();
        }
        self.session = None;
        self.incoming = None;
        self.active = None;
    }

    fn disconnect(&mut self) {
        if let Some(s) = &self.session {
            s.disconnect();
        }
        self.active = None;
    }

    fn poll(&mut self) {
        let mut events = Vec::new();
        if let Some(session) = &self.session {
            while let Ok(ev) = session.events().try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                HostEvent::Listening {
                    code,
                    addresses,
                    port,
                } => {
                    self.code = code;
                    self.addresses = addresses;
                    self.port = port.to_string();
                    self.error = None;
                }
                HostEvent::CodeRotated { code } => {
                    self.code = code;
                }
                HostEvent::Incoming {
                    peer,
                    client_name,
                    session_id,
                } => {
                    self.incoming = Some(Incoming {
                        peer,
                        client_name,
                        session_id,
                    });
                }
                HostEvent::IncomingCancelled => {
                    self.incoming = None;
                }
                HostEvent::SessionStarted { peer, session_id } => {
                    self.incoming = None;
                    self.warning = None;
                    self.active = Some(HostActive {
                        peer,
                        session_id,
                        fps: 0.0,
                        kbps: 0.0,
                        encode_ms: 0.0,
                        queue: 0,
                        bitrate_kbps: 0,
                    });
                }
                HostEvent::SessionEnded { reason } => {
                    self.active = None;
                    if reason != "local disconnect" {
                        self.warning = Some(format!("Session ended: {reason}"));
                    }
                }
                HostEvent::AuthFailed { peer, reason } => {
                    self.warning = Some(format!("Authentication failed from {peer}: {reason}"));
                }
                HostEvent::Stats {
                    fps,
                    kbps,
                    encode_ms,
                    queue,
                    bitrate_kbps,
                } => {
                    if let Some(active) = &mut self.active {
                        active.fps = fps;
                        active.kbps = kbps;
                        active.encode_ms = encode_ms;
                        active.queue = queue;
                        active.bitrate_kbps = bitrate_kbps;
                    }
                }
                HostEvent::Error { message } => {
                    self.error = Some(message);
                }
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        if self.session.is_none() {
            ui.heading("Host this PC");
            ui.label("Start listening, then give the viewer your address and one-time code.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Port");
                ui.add(
                    egui::TextEdit::singleline(&mut self.port)
                        .desired_width(90.0)
                        .char_limit(5),
                );
                if ui.button("Start listening").clicked() {
                    self.start();
                }
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "No installation, no service, no unattended access. You must accept every session.",
                )
                .weak(),
            );
            self.show_messages(ui);
            return;
        }

        let mut stop_clicked = false;
        let mut disconnect = false;

        if let Some(active) = &self.active {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(150, 26, 26))
                .inner_margin(egui::Margin::symmetric(14, 10))
                .corner_radius(6.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("● SESSION ACTIVE")
                                .color(egui::Color32::WHITE)
                                .strong()
                                .size(18.0),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("with {}", active.peer))
                                .color(egui::Color32::WHITE),
                        );
                    });
                    ui.label(
                        egui::RichText::new(
                            "Your screen and input are being shared. You can disconnect at any time.",
                        )
                        .color(egui::Color32::from_rgb(255, 220, 220)),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Elevated (admin) windows cannot receive injected input unless DeskBeam is run elevated.",
                        )
                        .small()
                        .color(egui::Color32::from_rgb(255, 220, 220)),
                    );
                });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(format!("Session {}", active.session_id));
                ui.separator();
                ui.label(format!("{:.0} fps", active.fps));
                ui.separator();
                ui.label(util::format_kbps(active.kbps));
                ui.separator();
                ui.label(format!("encode {:.1} ms", active.encode_ms));
                ui.separator();
                ui.label(format!("queue {}", active.queue));
                ui.separator();
                ui.label(format!("{} kbps target", active.bitrate_kbps));
            });
            ui.add_space(10.0);
            if ui
                .button(egui::RichText::new("Disconnect session").strong())
                .clicked()
            {
                disconnect = true;
            }
        } else {
            ui.heading("Waiting for a viewer");
            ui.add_space(6.0);

            ui.label("One-time access code");
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(28, 32, 40))
                .inner_margin(egui::Margin::symmetric(18, 14))
                .corner_radius(8.0)
                .show(ui, |ui| {
                    let code_text: &str = if self.code.is_empty() {
                        "--- --- ---"
                    } else {
                        self.code.as_str()
                    };
                    ui.label(
                        egui::RichText::new(code_text)
                        .monospace()
                        .size(38.0)
                        .strong()
                        .color(egui::Color32::from_rgb(120, 220, 170)),
                    );
                });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Single use. It rotates automatically after a session, rejection, or failed login.",
                )
                .weak(),
            );

            ui.add_space(12.0);
            ui.label("Reach this PC at");
            if self.addresses.is_empty() {
                ui.label(egui::RichText::new("No non-loopback IPv4 address found").weak());
            } else {
                for addr in &self.addresses {
                    ui.label(egui::RichText::new(addr).monospace());
                }
            }
            ui.label(
                egui::RichText::new(format!("TCP port {}", self.port))
                    .monospace()
                    .weak(),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "The viewer connects directly over LAN or VPN. No relay or cloud account is used.",
                )
                .weak(),
            );
            ui.add_space(12.0);
            if ui.button("Stop listening").clicked() {
                stop_clicked = true;
            }
        }

        self.show_messages(ui);

        if self.incoming.is_some() {
            let mut decision: Option<Decision> = None;
            egui::Window::new("Incoming connection")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    if let Some(inc) = &self.incoming {
                        ui.label(
                            egui::RichText::new(format!("{} wants to connect", inc.client_name))
                                .strong()
                                .size(17.0),
                        );
                        ui.label(format!("From: {}", inc.peer));
                        ui.label(format!("Session {}", inc.session_id));
                        ui.add_space(6.0);
                        ui.label("The access code was verified.");
                        ui.label(
                            egui::RichText::new(
                                "Only accept if you are expecting this connection. Accepting shares your screen and input.",
                            )
                            .weak(),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            if ui
                                .button(
                                    egui::RichText::new("Accept")
                                        .strong()
                                        .color(egui::Color32::from_rgb(120, 230, 160)),
                                )
                                .clicked()
                            {
                                decision = Some(Decision::Accept);
                            }
                            if ui
                                .button(
                                    egui::RichText::new("Reject")
                                        .color(egui::Color32::from_rgb(255, 140, 140)),
                                )
                                .clicked()
                            {
                                decision = Some(Decision::Reject);
                            }
                        });
                    }
                });
            if let Some(d) = decision {
                if let Some(s) = &self.session {
                    match d {
                        Decision::Accept => s.accept(),
                        Decision::Reject => s.reject(),
                    }
                }
                self.incoming = None;
            }
        }

        if stop_clicked {
            self.stop();
        }
        if disconnect {
            self.disconnect();
        }
        if self.session.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn show_messages(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = &self.error {
            ui.add_space(10.0);
            ui.colored_label(egui::Color32::from_rgb(255, 120, 120), error.as_str());
        }
        if let Some(warning) = &self.warning {
            ui.add_space(10.0);
            ui.colored_label(egui::Color32::from_rgb(255, 200, 120), warning);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewerState {
    Idle,
    Connecting,
    Waiting,
    Streaming,
}

struct ViewerUi {
    address: String,
    code: String,
    session: Option<ViewerSession>,
    state: ViewerState,
    status: String,
    info: Option<SessionInfo>,
    texture: Option<egui::TextureHandle>,
    frame_size: Option<(usize, usize)>,
    stats: ViewerStats,
    monitors: Vec<MonitorInfo>,
    selected_monitor: usize,
    keyboard_enabled: bool,
    focus_stream: bool,
    prev_modifiers: egui::Modifiers,
    error: Option<String>,
}

impl Default for ViewerUi {
    fn default() -> Self {
        Self {
            address: String::new(),
            code: String::new(),
            session: None,
            state: ViewerState::Idle,
            status: String::new(),
            info: None,
            texture: None,
            frame_size: None,
            stats: ViewerStats {
                fps: 0.0,
                kbps: 0.0,
                rtt_ms: 0,
                decode_ms: 0.0,
            },
            monitors: Vec::new(),
            selected_monitor: 0,
            keyboard_enabled: true,
            focus_stream: false,
            prev_modifiers: egui::Modifiers::default(),
            error: None,
        }
    }
}

impl ViewerUi {
    fn connect(&mut self) {
        if self.session.is_some() {
            return;
        }
        let address = normalize_address(&self.address);
        if address.is_empty() {
            self.error = Some("Enter the host address.".into());
            return;
        }
        if self.code.chars().filter(|c| c.is_ascii_digit()).count() != 9 {
            self.error = Some("The access code must contain 9 digits.".into());
            return;
        }
        self.error = None;
        self.info = None;
        self.texture = None;
        self.frame_size = None;
        self.state = ViewerState::Connecting;
        self.status = "Connecting…".into();
        self.session = Some(ViewerSession::connect(&address, &self.code));
    }

    fn disconnect(&mut self) {
        if let Some(s) = &self.session {
            s.disconnect();
        }
        self.session = None;
        self.state = ViewerState::Idle;
        self.texture = None;
        self.frame_size = None;
        self.focus_stream = false;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        let mut latest_frame = None;
        if let Some(session) = &self.session {
            while let Ok(ev) = session.events.try_recv() {
                events.push(ev);
            }
            while let Ok(frame) = session.frames.try_recv() {
                latest_frame = Some(frame);
            }
        }

        for ev in events {
            match ev {
                ViewerEvent::Status(message) => {
                    self.status = message.clone();
                    if self.state == ViewerState::Connecting {
                        self.state = ViewerState::Waiting;
                    }
                }
                ViewerEvent::Ready(info) => {
                    self.monitors = info.monitors.clone();
                    self.selected_monitor = self
                        .monitors
                        .iter()
                        .position(|m| m.primary)
                        .unwrap_or(0);
                    self.info = Some(*info);
                    self.state = ViewerState::Streaming;
                    self.keyboard_enabled = true;
                    self.focus_stream = false;
                    self.status = "Session active".into();
                    self.error = None;
                }
                ViewerEvent::Stats(stats) => {
                    self.stats = stats;
                }
                ViewerEvent::Closed(reason) => {
                    if self.state != ViewerState::Idle {
                        self.error = Some(format!("Session closed: {reason}"));
                    }
                    self.session = None;
                    self.state = ViewerState::Idle;
                    self.texture = None;
                    self.info = None;
                }
                ViewerEvent::Error(message) => {
                    self.error = Some(message);
                    self.session = None;
                    self.state = ViewerState::Idle;
                    self.texture = None;
                    self.info = None;
                }
            }
        }

        if let Some(frame) = latest_frame {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width, frame.height],
                &frame.rgba,
            );
            match &mut self.texture {
                Some(texture) if texture.size() == [frame.width, frame.height] => {
                    texture.set(image, egui::TextureOptions::LINEAR);
                }
                _ => {
                    self.texture =
                        Some(ctx.load_texture("remote-screen", image, egui::TextureOptions::LINEAR));
                }
            }
            self.frame_size = Some((frame.width, frame.height));
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        match self.state {
            ViewerState::Idle | ViewerState::Connecting | ViewerState::Waiting => {
                self.form_ui(ui);
            }
            ViewerState::Streaming => {
                self.stream_ui(ui);
            }
        }
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Connect to a host");
        ui.label("Enter the host's LAN/VPN address and the one-time code it displays.");
        ui.add_space(10.0);

        let busy = self.state != ViewerState::Idle;
        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.address)
                    .hint_text("192.168.1.20:7878")
                    .desired_width(260.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Code   ");
            ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.code)
                    .hint_text("123-456-789")
                    .desired_width(180.0)
                    .char_limit(11),
            );
        });
        ui.add_space(10.0);

        if !busy {
            if ui.button("Connect").clicked() {
                self.connect();
            }
            if !self.status.is_empty() {
                ui.add_space(6.0);
                ui.label(self.status.as_str());
            }
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(self.status.as_str());
            });
            ui.add_space(8.0);
            if ui.button("Cancel").clicked() {
                self.disconnect();
            }
        }

        if let Some(error) = &self.error {
            ui.add_space(10.0);
            ui.colored_label(egui::Color32::from_rgb(255, 120, 120), error.as_str());
        }
        if self.state != ViewerState::Idle {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    fn stream_ui(&mut self, ui: &mut egui::Ui) {
        let mut disconnect = false;
        let mut toggle_keyboard = false;

        ui.horizontal(|ui| {
            if ui
                .button(egui::RichText::new("Disconnect").color(egui::Color32::from_rgb(255, 140, 140)))
                .clicked()
            {
                disconnect = true;
            }
            ui.separator();
            if let Some(monitor) = self.monitors.get(self.selected_monitor) {
                ui.label(format!("Monitor: {}", monitor.name))
                    .on_hover_text(
                        "M1 streams the primary monitor. Multi-monitor selection is milestone M4.",
                    );
                ui.separator();
            }
            ui.label(format!("{:.0} fps", self.stats.fps));
            ui.label(util::format_kbps(self.stats.kbps));
            ui.label(format!("RTT {} ms", self.stats.rtt_ms));
            ui.label(format!("decode {:.1} ms", self.stats.decode_ms));
            ui.separator();
            let label = if self.keyboard_enabled {
                "Keyboard: remote"
            } else {
                "Keyboard: local"
            };
            if ui.button(label).clicked() {
                toggle_keyboard = true;
            }
            if !self.keyboard_enabled {
                ui.label(
                    egui::RichText::new("Click the stream to send keyboard input remotely.")
                        .weak(),
                );
            }
        });

        if toggle_keyboard {
            self.keyboard_enabled = !self.keyboard_enabled;
            if !self.keyboard_enabled {
                self.release_modifiers();
            }
        }
        if disconnect {
            self.disconnect();
            return;
        }

        let available = ui.available_size();
        let (rect, _response) =
            ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::BLACK);

        let frame_size = self.frame_size;
        let mut image_rect = rect;
        if let (Some(texture), Some((fw, fh))) = (&self.texture, frame_size) {
            if fw > 0 && fh > 0 {
                let scale = (rect.width() / fw as f32)
                    .min(rect.height() / fh as f32)
                    .max(0.01);
                let size = egui::vec2(fw as f32 * scale, fh as f32 * scale);
                image_rect = egui::Rect::from_center_size(rect.center(), size);
                painter.image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Waiting for video…",
                egui::FontId::proportional(16.0),
                egui::Color32::GRAY,
            );
        }

        if self.session.is_some() {
            let events = ui.input(|i| i.events.clone());
            let mut sent_move = false;
            for event in events {
                match event {
                    egui::Event::PointerMoved(pos) => {
                        if image_rect.contains(pos) && !sent_move {
                            if let Some((nx, ny)) = normalize_pointer(pos, image_rect) {
                                self.send_input(InputEvent::MouseMove { nx, ny });
                                sent_move = true;
                            }
                        }
                    }
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        if image_rect.contains(pos) {
                            if let Some((nx, ny)) = normalize_pointer(pos, image_rect) {
                                self.send_input(InputEvent::MouseMove { nx, ny });
                            }
                            if let Some(b) = pointer_button(button) {
                                self.send_input(InputEvent::MouseButton { button: b, down: pressed });
                            }
                            if pressed {
                                self.focus_stream = true;
                                self.keyboard_enabled = true;
                            }
                        } else if pressed {
                            // Clicking the toolbar or elsewhere releases remote input.
                            if self.focus_stream {
                                self.release_modifiers();
                            }
                            self.focus_stream = false;
                        }
                    }
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        if image_rect.contains(
                            ui.input(|i| i.pointer.hover_pos()).unwrap_or(egui::Pos2::ZERO),
                        ) {
                            let (dx, dy) = wheel_steps(unit, delta);
                            if dx != 0 || dy != 0 {
                                self.send_input(InputEvent::Wheel { dx, dy });
                            }
                        }
                    }
                    egui::Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } => {
                        if self.keyboard_enabled && self.focus_stream {
                            let printable = is_printable_key(key);
                            if !printable || modifiers.ctrl || modifiers.alt || modifiers.command {
                                if let Some((vk, extended)) = key_to_vk(key) {
                                    self.send_input(InputEvent::Key {
                                        vk,
                                        down: pressed,
                                        extended,
                                    });
                                }
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        let modifiers = ui.input(|i| i.modifiers);
                        if self.keyboard_enabled
                            && self.focus_stream
                            && !modifiers.ctrl
                            && !modifiers.alt
                            && !modifiers.command
                            && !text.is_empty()
                            && !text.chars().all(|c| c.is_control())
                        {
                            self.send_input(InputEvent::Text { text });
                        }
                    }
                    _ => {}
                }
            }

            let modifiers = ui.input(|i| i.modifiers);
            if self.keyboard_enabled && self.focus_stream && modifiers != self.prev_modifiers {
                self.send_modifier_changes(&modifiers);
            }
            self.prev_modifiers = modifiers;
        }

        if let Some(error) = &self.error {
            ui.add_space(6.0);
            ui.colored_label(egui::Color32::from_rgb(255, 120, 120), error.as_str());
        }

        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }

    fn send_input(&self, event: InputEvent) {
        if let Some(session) = &self.session {
            session.send_input(event);
        }
    }

    fn send_modifier_changes(&self, now: &egui::Modifiers) {
        let pairs = [
            (self.prev_modifiers.shift, now.shift, 0x10u16),
            (self.prev_modifiers.ctrl, now.ctrl, 0x11),
            (self.prev_modifiers.alt, now.alt, 0x12),
            // `command` aliases Ctrl on Windows; sending a Win-key event for it
            // would inject an unintended modifier.
        ];
        for (was, is, vk) in pairs {
            if was != is {
                self.send_input(InputEvent::Key {
                    vk,
                    down: is,
                    extended: false,
                });
            }
        }
    }

    fn release_modifiers(&self) {
        for vk in [0x10u16, 0x11, 0x12] {
            self.send_input(InputEvent::Key {
                vk,
                down: false,
                extended: false,
            });
        }
    }
}

pub struct DeskBeamApp {
    tab: Tab,
    host: HostUi,
    viewer: ViewerUi,
    title_active: bool,
}

impl DeskBeamApp {
    pub fn new(cc: &eframe::CreationContext<'_>, mode: StartMode) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::from_rgb(16, 18, 22);
        visuals.window_fill = egui::Color32::from_rgb(24, 27, 33);
        visuals.extreme_bg_color = egui::Color32::from_rgb(10, 12, 15);
        visuals.override_text_color = Some(egui::Color32::from_rgb(226, 230, 238));
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.spacing.item_spacing = egui::vec2(10.0, 8.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
        });

        let mut app = Self {
            tab: Tab::Host,
            host: HostUi::default(),
            viewer: ViewerUi::default(),
            title_active: false,
        };

        match mode {
            StartMode::Gui => {}
            StartMode::Host { port } => {
                app.host.port = port.to_string();
                app.host.start();
            }
            StartMode::Viewer { address, code } => {
                app.tab = Tab::Viewer;
                app.viewer.address = address;
                if let Some(code) = code {
                    app.viewer.code = code;
                }
                app.viewer.connect();
            }
        }
        app
    }
}

impl eframe::App for DeskBeamApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.host.poll();
        self.viewer.poll(ctx);

        let active = self.host.active.is_some();
        if active != self.title_active {
            self.title_active = active;
            let title = if active {
                "DeskBeam - SESSION ACTIVE"
            } else {
                "DeskBeam"
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_string()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("DeskBeam")
                        .size(22.0)
                        .strong()
                        .color(egui::Color32::from_rgb(120, 210, 255)),
                );
                ui.label(
                    egui::RichText::new("direct · encrypted · consent-first")
                        .weak()
                        .size(12.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.tab, Tab::Viewer, "Connect");
                    ui.selectable_value(&mut self.tab, Tab::Host, "Host");
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Host => self.host.ui(ui),
            Tab::Viewer => self.viewer.ui(ui),
        });

        if self.host.session.is_some() || self.viewer.session.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
    }
}

fn normalize_address(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }
    // Hostnames without a port get the default. IPv6 is out of scope for M1.
    if input.contains(':') {
        input.to_string()
    } else {
        format!("{input}:{DEFAULT_PORT}")
    }
}

fn normalize_pointer(pos: egui::Pos2, rect: egui::Rect) -> Option<(u16, u16)> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let nx = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let ny = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    Some(((nx * 65_535.0) as u16, (ny * 65_535.0) as u16))
}

fn pointer_button(button: egui::PointerButton) -> Option<u8> {
    Some(match button {
        egui::PointerButton::Primary => 0,
        egui::PointerButton::Secondary => 1,
        egui::PointerButton::Middle => 2,
        _ => return None,
    })
}

fn wheel_steps(unit: egui::MouseWheelUnit, delta: egui::Vec2) -> (i16, i16) {
    let scale = match unit {
        egui::MouseWheelUnit::Line => 1.0,
        egui::MouseWheelUnit::Point => 1.0 / 40.0,
        egui::MouseWheelUnit::Page => 10.0,
    };
    let dx = (delta.x * scale).round().clamp(-20.0, 20.0) as i16;
    let dy = (delta.y * scale).round().clamp(-20.0, 20.0) as i16;
    (dx, dy)
}

fn is_printable_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::A
            | egui::Key::B
            | egui::Key::C
            | egui::Key::D
            | egui::Key::E
            | egui::Key::F
            | egui::Key::G
            | egui::Key::H
            | egui::Key::I
            | egui::Key::J
            | egui::Key::K
            | egui::Key::L
            | egui::Key::M
            | egui::Key::N
            | egui::Key::O
            | egui::Key::P
            | egui::Key::Q
            | egui::Key::R
            | egui::Key::S
            | egui::Key::T
            | egui::Key::U
            | egui::Key::V
            | egui::Key::W
            | egui::Key::X
            | egui::Key::Y
            | egui::Key::Z
            | egui::Key::Num0
            | egui::Key::Num1
            | egui::Key::Num2
            | egui::Key::Num3
            | egui::Key::Num4
            | egui::Key::Num5
            | egui::Key::Num6
            | egui::Key::Num7
            | egui::Key::Num8
            | egui::Key::Num9
            | egui::Key::Space
            | egui::Key::Minus
            | egui::Key::Plus
            | egui::Key::Equals
            | egui::Key::OpenBracket
            | egui::Key::CloseBracket
            | egui::Key::Backslash
            | egui::Key::Semicolon
            | egui::Key::Quote
            | egui::Key::Comma
            | egui::Key::Period
            | egui::Key::Slash
            | egui::Key::Backtick
            | egui::Key::Colon
            | egui::Key::Pipe
            | egui::Key::Questionmark
            | egui::Key::Exclamationmark
            | egui::Key::OpenCurlyBracket
            | egui::Key::CloseCurlyBracket
    )
}

fn key_to_vk(key: egui::Key) -> Option<(u16, bool)> {
    use egui::Key as K;
    Some(match key {
        K::A => (0x41, false),
        K::B => (0x42, false),
        K::C => (0x43, false),
        K::D => (0x44, false),
        K::E => (0x45, false),
        K::F => (0x46, false),
        K::G => (0x47, false),
        K::H => (0x48, false),
        K::I => (0x49, false),
        K::J => (0x4A, false),
        K::K => (0x4B, false),
        K::L => (0x4C, false),
        K::M => (0x4D, false),
        K::N => (0x4E, false),
        K::O => (0x4F, false),
        K::P => (0x50, false),
        K::Q => (0x51, false),
        K::R => (0x52, false),
        K::S => (0x53, false),
        K::T => (0x54, false),
        K::U => (0x55, false),
        K::V => (0x56, false),
        K::W => (0x57, false),
        K::X => (0x58, false),
        K::Y => (0x59, false),
        K::Z => (0x5A, false),
        K::Num0 => (0x30, false),
        K::Num1 => (0x31, false),
        K::Num2 => (0x32, false),
        K::Num3 => (0x33, false),
        K::Num4 => (0x34, false),
        K::Num5 => (0x35, false),
        K::Num6 => (0x36, false),
        K::Num7 => (0x37, false),
        K::Num8 => (0x38, false),
        K::Num9 => (0x39, false),
        K::F1 => (0x70, false),
        K::F2 => (0x71, false),
        K::F3 => (0x72, false),
        K::F4 => (0x73, false),
        K::F5 => (0x74, false),
        K::F6 => (0x75, false),
        K::F7 => (0x76, false),
        K::F8 => (0x77, false),
        K::F9 => (0x78, false),
        K::F10 => (0x79, false),
        K::F11 => (0x7A, false),
        K::F12 => (0x7B, false),
        K::Escape => (0x1B, false),
        K::Tab => (0x09, false),
        K::Backspace => (0x08, false),
        K::Enter => (0x0D, false),
        K::Space => (0x20, false),
        K::Insert => (0x2D, true),
        K::Delete => (0x2E, true),
        K::Home => (0x24, true),
        K::End => (0x23, true),
        K::PageUp => (0x21, true),
        K::PageDown => (0x22, true),
        K::ArrowLeft => (0x25, true),
        K::ArrowUp => (0x26, true),
        K::ArrowRight => (0x27, true),
        K::ArrowDown => (0x28, true),
        K::Minus => (0xBD, false),
        K::Plus | K::Equals => (0xBB, false),
        K::OpenBracket => (0xDB, false),
        K::CloseBracket => (0xDD, false),
        K::Backslash => (0xDC, false),
        K::Semicolon | K::Colon => (0xBA, false),
        K::Quote => (0xDE, false),
        K::Comma => (0xBC, false),
        K::Period => (0xBE, false),
        K::Slash => (0xBF, false),
        K::Backtick => (0xC0, false),
        _ => return None,
    })
}
