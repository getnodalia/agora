//! Security: two-step verification (an authenticator app) and this person's devices.
//!
//! Requests run on the network runtime and come back as [`Reply::Security`]; this module only
//! keeps the dialog's state, draws it and starts requests.

use agora_client::Client;
use agora_protocol::{MfaSetup, MfaStatus, OwnDevice, RecoveryStatus};
use egui::{vec2, Align, Layout, RichText, Sense, TextEdit, Ui};
use nodalia_theme::creator::{self as cr, Look};
use nodalia_theme::font_size;
use nodalia_theme::for_egui::color;

use crate::app::Action;
use crate::i18n::{tr, trf};
use crate::net::{Net, Reply, Result};
use crate::signin::status_line;

#[derive(Clone, Debug, Default)]
pub enum Stage {
    /// Status and devices.
    #[default]
    Overview,
    /// Asking the password before a new secret is made.
    SetupPassword,
    /// Showing the secret; waiting for the first code.
    Scan(MfaSetup),
    /// New recovery codes, shown once.
    Codes(Vec<String>),
    /// Asking the password and a code to turn it off.
    Disable,
    /// Asking a code for new recovery codes.
    NewCodes,
    /// Asking the old and the new password.
    ChangePassword,
    /// Asking the password before a new recovery key is made.
    KeyPassword,
    /// A recovery key, shown once.
    Key(String),
}

#[derive(Clone, Debug, Default)]
pub struct SecurityDialog {
    pub status: Option<MfaStatus>,
    pub recovery: Option<RecoveryStatus>,
    pub devices: Vec<OwnDevice>,
    pub stage: Stage,
    pub password: String,
    pub new_password: String,
    pub code: String,
    pub pending: bool,
    pub error: Option<String>,
}

/// What the dialog asked for.
#[derive(Clone, Debug)]
pub enum SecurityAction {
    Load,
    Setup,
    Enable,
    Disable,
    NewCodes,
    ChangePassword,
    NewKey,
    RemoveDevice(String),
    Stage(Stage),
}

/// What a request answered.
#[derive(Debug)]
pub enum SecurityReply {
    Loaded(Result<(MfaStatus, RecoveryStatus, Vec<OwnDevice>)>),
    Setup(Result<MfaSetup>),
    Codes(Result<Vec<String>>),
    Key(Result<String>),
    Done(Result<()>),
}

impl SecurityDialog {
    /// Start what `action` asks for. `false` when nothing is to be done.
    pub fn act(&mut self, net: &Net, client: &Client, action: SecurityAction) {
        if let SecurityAction::Stage(stage) = action {
            self.stage = stage;
            self.password.clear();
            self.new_password.clear();
            self.code.clear();
            self.error = None;
            return;
        }
        if self.pending {
            return;
        }
        self.pending = true;
        self.error = None;
        let client = client.clone();
        let password = std::mem::take(&mut self.password);
        let new_password = std::mem::take(&mut self.new_password);
        let code = std::mem::take(&mut self.code);
        match action {
            SecurityAction::Load => net.run(async move {
                let result = async {
                    Ok((
                        client.mfa_status().await?,
                        client.recovery_status().await?,
                        client.own_devices().await?,
                    ))
                }
                .await;
                Reply::Security(SecurityReply::Loaded(result))
            }),
            SecurityAction::Setup => net.run(async move {
                Reply::Security(SecurityReply::Setup(client.mfa_setup(&password).await))
            }),
            SecurityAction::Enable => net.run(async move {
                let result = client.mfa_enable(&code).await.map(|c| c.recovery_codes);
                Reply::Security(SecurityReply::Codes(result))
            }),
            SecurityAction::NewCodes => net.run(async move {
                let result = client
                    .mfa_new_recovery_codes(&code)
                    .await
                    .map(|c| c.recovery_codes);
                Reply::Security(SecurityReply::Codes(result))
            }),
            SecurityAction::Disable => net.run(async move {
                Reply::Security(SecurityReply::Done(
                    client.mfa_disable(&password, &code).await,
                ))
            }),
            SecurityAction::ChangePassword => net.run(async move {
                Reply::Security(SecurityReply::Done(
                    client.change_password(&password, &new_password).await,
                ))
            }),
            SecurityAction::NewKey => net.run(async move {
                let result = client.new_recovery_key(&password).await;
                Reply::Security(SecurityReply::Key(result.map(|k| k.recovery_key)))
            }),
            SecurityAction::RemoveDevice(key) => net.run(async move {
                Reply::Security(SecurityReply::Done(client.remove_device(&key).await))
            }),
            SecurityAction::Stage(_) => {}
        }
    }

    /// Fold an answer in. Returns `true` when the dialog should reload.
    pub fn answer(&mut self, reply: SecurityReply) -> bool {
        self.pending = false;
        match reply {
            SecurityReply::Loaded(Ok((status, recovery, devices))) => {
                self.status = Some(status);
                self.recovery = Some(recovery);
                self.devices = devices;
                false
            }
            SecurityReply::Setup(Ok(setup)) => {
                self.stage = Stage::Scan(setup);
                false
            }
            SecurityReply::Codes(Ok(codes)) => {
                self.stage = Stage::Codes(codes);
                true
            }
            SecurityReply::Key(Ok(key)) => {
                self.stage = Stage::Key(key);
                true
            }
            SecurityReply::Done(Ok(())) => {
                self.stage = Stage::Overview;
                true
            }
            SecurityReply::Loaded(Err(e))
            | SecurityReply::Setup(Err(e))
            | SecurityReply::Codes(Err(e))
            | SecurityReply::Key(Err(e))
            | SecurityReply::Done(Err(e)) => {
                self.error = Some(describe(&e));
                false
            }
        }
    }
}

fn describe(e: &agora_client::ClientError) -> String {
    use agora_protocol::ErrorCode;
    match e.code() {
        Some(ErrorCode::Unauthenticated) => tr("The password is not right.").to_string(),
        Some(ErrorCode::Invalid) => {
            tr("That code is not valid. Check the time on this computer and try again.").to_string()
        }
        Some(ErrorCode::RateLimited) => tr("Too many attempts. Wait a minute.").to_string(),
        _ => crate::i18n::explain(e),
    }
}

pub fn show(ui: &mut Ui, look: &Look, dialog: &mut SecurityDialog, actions: &mut Vec<Action>) {
    let p = &look.palette;
    let mut wanted: Vec<SecurityAction> = Vec::new();
    let mut act = |a: SecurityAction| wanted.push(a);
    if let Some(error) = &dialog.error {
        status_line(ui, look, error, true);
        ui.add_space(8.0);
    }
    match dialog.stage.clone() {
        Stage::Overview => {
            heading(ui, look, tr("Password and recovery"));
            let has_key = dialog.recovery.as_ref().is_some_and(|r| r.has_key);
            note(
                ui,
                look,
                if has_key {
                    tr("You have a recovery key. If you forget your password, it lets you choose a new one from the sign-in screen.")
                } else {
                    tr("You have no recovery key yet. Without one, only whoever runs this server can help you if you forget your password.")
                },
            );
            ui.horizontal(|ui| {
                if cr::secondary_button(ui, look, tr("Change password"), !dialog.pending).clicked()
                {
                    act(SecurityAction::Stage(Stage::ChangePassword));
                }
                let key_label = if has_key {
                    tr("New recovery key")
                } else {
                    tr("Create a recovery key")
                };
                if cr::secondary_button(ui, look, key_label, !dialog.pending).clicked() {
                    act(SecurityAction::Stage(Stage::KeyPassword));
                }
            });
            ui.add_space(12.0);
            heading(ui, look, tr("Two-step verification"));
            match &dialog.status {
                None => {
                    ui.spinner();
                }
                Some(status) if status.enabled => {
                    note(
                        ui,
                        look,
                        &trf(
                            "On. Signing in asks for a code from your authenticator app. Recovery codes left: {}.",
                            &[&status.recovery_codes_left.to_string()],
                        ),
                    );
                    ui.horizontal(|ui| {
                        if cr::secondary_button(ui, look, tr("New recovery codes"), !dialog.pending)
                            .clicked()
                        {
                            act(SecurityAction::Stage(Stage::NewCodes));
                        }
                        if cr::secondary_button(ui, look, tr("Turn off"), !dialog.pending).clicked()
                        {
                            act(SecurityAction::Stage(Stage::Disable));
                        }
                    });
                }
                Some(_) => {
                    note(
                        ui,
                        look,
                        tr("Off. Add a code from an authenticator app to your password, so a stolen password is not enough to sign in."),
                    );
                    if cr::primary_button(ui, look, tr("Turn on"), !dialog.pending).clicked() {
                        act(SecurityAction::Stage(Stage::SetupPassword));
                    }
                }
            }
            ui.add_space(12.0);
            heading(ui, look, tr("Your devices"));
            note(
                ui,
                look,
                tr("Each computer you sign in on can read your conversations. Logging out keeps a computer's place; remove the ones you no longer use."),
            );
            let mut remove = None;
            for device in &dialog.devices {
                cr::island(look).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            let fingerprint = base64_fingerprint(&device.public_key);
                            let title = if device.this_device {
                                tr("This computer")
                            } else if device.signed_in {
                                tr("Signed in")
                            } else {
                                tr("Signed out")
                            };
                            ui.label(RichText::new(title).strong().color(color(p.text_primary)));
                            ui.label(
                                RichText::new(fingerprint)
                                    .monospace()
                                    .size(font_size::CAPTION)
                                    .color(color(p.text_secondary)),
                            );
                            ui.label(
                                RichText::new(trf(
                                    "Last seen {}",
                                    &[&crate::view::date(device.last_seen_at)],
                                ))
                                .size(font_size::CAPTION)
                                .color(color(p.text_tertiary)),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if !device.this_device
                                && cr::secondary_button(ui, look, tr("Remove"), !dialog.pending)
                                    .clicked()
                            {
                                remove = Some(device.public_key.clone());
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
            if let Some(key) = remove {
                act(SecurityAction::RemoveDevice(key));
            }
        }
        Stage::SetupPassword => {
            note(ui, look, tr("Enter your password to continue."));
            label(ui, look, tr("Password"));
            let enter = field(ui, look, &mut dialog.password, true);
            let ready = !dialog.password.is_empty() && !dialog.pending;
            if footer(ui, look, tr("Continue"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::Setup);
            }
        }
        Stage::Scan(setup) => {
            note(
                ui,
                look,
                tr("Scan this code with an authenticator app (any app that supports TOTP), or type the key. Then enter the six digits it shows."),
            );
            ui.vertical_centered(|ui| qr(ui, &setup.uri, 180.0));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(group(&setup.secret))
                        .monospace()
                        .color(color(p.text_primary)),
                );
                if ui.small_button(tr("Copy")).clicked() {
                    actions.push(Action::Copy(setup.secret.clone()));
                }
            });
            label(ui, look, tr("Code from your authenticator app"));
            let enter = field(ui, look, &mut dialog.code, false);
            let ready = dialog.code.trim().len() >= 6 && !dialog.pending;
            if footer(ui, look, tr("Turn on"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::Enable);
            }
        }
        Stage::Codes(codes) => {
            note(
                ui,
                look,
                tr("Keep these recovery codes somewhere safe. Each one signs you in once if you lose your authenticator app. They are shown only now."),
            );
            egui::Grid::new("recovery-codes")
                .num_columns(2)
                .spacing(vec2(16.0, 4.0))
                .show(ui, |ui| {
                    for pair in codes.chunks(2) {
                        for code in pair {
                            ui.label(RichText::new(code).monospace().color(color(p.text_primary)));
                        }
                        ui.end_row();
                    }
                });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if cr::secondary_button(ui, look, tr("Copy"), true).clicked() {
                    actions.push(Action::Copy(codes.join("\n")));
                }
                if cr::primary_button(ui, look, tr("Done"), true).clicked() {
                    act(SecurityAction::Stage(Stage::Overview));
                }
            });
        }
        Stage::Disable => {
            note(
                ui,
                look,
                tr("Enter your password and a current code to turn two-step verification off."),
            );
            label(ui, look, tr("Password"));
            field(ui, look, &mut dialog.password, true);
            label(ui, look, tr("Code from your authenticator app"));
            let enter = field(ui, look, &mut dialog.code, false);
            let ready =
                !dialog.password.is_empty() && dialog.code.trim().len() >= 6 && !dialog.pending;
            if footer(ui, look, tr("Turn off"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::Disable);
            }
        }
        Stage::ChangePassword => {
            note(
                ui,
                look,
                tr("Other computers and sessions are signed out when you change it. Your conversations stay readable."),
            );
            label(ui, look, tr("Current password"));
            field(ui, look, &mut dialog.password, true);
            label(ui, look, tr("New password"));
            let enter = field(ui, look, &mut dialog.new_password, true);
            let ready = !dialog.password.is_empty()
                && dialog.new_password.chars().count() >= agora_core::limits::PASSWORD_MIN
                && !dialog.pending;
            ui.label(
                RichText::new(trf(
                    "At least {} characters.",
                    &[&agora_core::limits::PASSWORD_MIN.to_string()],
                ))
                .size(font_size::CAPTION)
                .color(color(p.text_tertiary)),
            );
            if footer(ui, look, tr("Change password"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::ChangePassword);
            }
        }
        Stage::KeyPassword => {
            note(
                ui,
                look,
                tr("Enter your password to make a new recovery key. The old one stops working."),
            );
            label(ui, look, tr("Password"));
            let enter = field(ui, look, &mut dialog.password, true);
            let ready = !dialog.password.is_empty() && !dialog.pending;
            if footer(ui, look, tr("Continue"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::NewKey);
            }
        }
        Stage::Key(key) => {
            note(
                ui,
                look,
                tr("This is your recovery key. If you forget your password, choose “Forgot your password?” on the sign-in screen and enter it. Keep it somewhere safe, apart from your password. It is shown only now."),
            );
            cr::island(look).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(key.replace('-', " "))
                            .monospace()
                            .size(font_size::BODY)
                            .strong()
                            .color(color(p.text_primary)),
                    );
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if cr::secondary_button(ui, look, tr("Copy"), true).clicked() {
                    actions.push(Action::Copy(key.clone()));
                }
                if cr::primary_button(ui, look, tr("I have saved it"), true).clicked() {
                    act(SecurityAction::Stage(Stage::Overview));
                }
            });
        }
        Stage::NewCodes => {
            note(
                ui,
                look,
                tr("Enter a current code. Your old recovery codes stop working."),
            );
            label(ui, look, tr("Code from your authenticator app"));
            let enter = field(ui, look, &mut dialog.code, false);
            let ready = dialog.code.trim().len() >= 6 && !dialog.pending;
            if footer(ui, look, tr("Continue"), ready, &mut act) || (enter && ready) {
                act(SecurityAction::NewCodes);
            }
        }
    }
    if dialog.pending {
        ui.add_space(6.0);
        ui.spinner();
    }
    actions.extend(wanted.into_iter().map(Action::Security));
}

fn heading(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(
        RichText::new(text)
            .strong()
            .color(color(look.palette.text_primary)),
    );
    ui.add_space(4.0);
}

fn note(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(
        RichText::new(text)
            .size(font_size::SMALL)
            .color(color(look.palette.text_secondary)),
    );
    ui.add_space(6.0);
}

fn label(ui: &mut Ui, look: &Look, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text)
            .size(font_size::SMALL)
            .color(color(look.palette.text_secondary)),
    );
}

/// A one-line field. `true` when Enter was pressed in it.
fn field(ui: &mut Ui, look: &Look, text: &mut String, password: bool) -> bool {
    let response = cr::field_frame(look)
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.add(
                TextEdit::singleline(text)
                    .frame(egui::Frame::NONE)
                    .align(egui::Align2::LEFT_CENTER)
                    .password(password)
                    .desired_width(ui.available_width()),
            )
        })
        .inner;
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Back to the overview, and the confirming button. `true` when the confirming one was clicked;
/// Back asks for the overview through `act`.
fn footer(
    ui: &mut Ui,
    look: &Look,
    confirm: &str,
    enabled: bool,
    act: &mut impl FnMut(SecurityAction),
) -> bool {
    ui.add_space(12.0);
    let mut clicked = false;
    let row = vec2(ui.available_width(), 32.0);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        clicked = cr::primary_button(ui, look, confirm, enabled).clicked();
        if cr::secondary_button(ui, look, tr("Back"), true).clicked() {
            act(SecurityAction::Stage(Stage::Overview));
        }
    });
    clicked
}

/// Groups of four for reading a key aloud or typing it.
fn group(secret: &str) -> String {
    secret
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn base64_fingerprint(key: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(key)
        .map(|k| agora_e2e::fingerprint(&k))
        .unwrap_or_default()
}

/// Draw `text` as a QR code, dark on a light square whatever the theme (scanners need it).
fn qr(ui: &mut Ui, text: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return;
    };
    let width = code.width();
    let quiet = 2;
    let cells = width + quiet * 2;
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, egui::Color32::WHITE);
    let cell = size / cells as f32;
    let colors = code.to_colors();
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = (i % width + quiet, i / width + quiet);
            let min = rect.min + vec2(x as f32 * cell, y as f32 * cell);
            painter.rect_filled(
                egui::Rect::from_min_size(min, vec2(cell + 0.5, cell + 0.5)),
                0.0,
                egui::Color32::BLACK,
            );
        }
    }
}
