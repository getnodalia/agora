//! Integrations: other apps (Hito, a script, a build server) that post notices to a channel
//! through a signed web address. Admins add and remove them; everyone in the channel sees them.
//!
//! Requests run on the network runtime and come back as [`Reply::Integrations`]; this module only
//! keeps the dialog's state, draws it and starts requests.

use agora_client::Client;
use agora_core::ChannelId;
use agora_protocol::{CreateIntegrationRequest, Integration, IntegrationKind};
use egui::{vec2, Align, Layout, RichText, TextEdit, Ui};
use nodalia_theme::creator::{self as cr, Look};
use nodalia_theme::font_size;
use nodalia_theme::for_egui::color;

use crate::app::Action;
use crate::i18n::{tr, trf};
use crate::net::{Net, Reply, Result};
use crate::signin::status_line;

/// The address and key of an integration just made. Shown once.
#[derive(Clone, Debug)]
pub struct Created {
    pub name: String,
    pub kind: IntegrationKind,
    pub url: String,
    pub secret: String,
}

#[derive(Clone, Debug)]
pub struct IntegrationsDialog {
    pub channel: ChannelId,
    /// Whether this person can add and remove integrations here.
    pub admin: bool,
    pub list: Option<Vec<Integration>>,
    pub kind: IntegrationKind,
    pub name: String,
    pub created: Option<Created>,
    pub pending: bool,
    pub error: Option<String>,
}

impl IntegrationsDialog {
    pub fn new(channel: ChannelId, admin: bool) -> IntegrationsDialog {
        IntegrationsDialog {
            channel,
            admin,
            list: None,
            kind: IntegrationKind::Hito,
            name: String::new(),
            created: None,
            pending: false,
            error: None,
        }
    }
}

/// What the dialog asked for.
#[derive(Clone, Debug)]
pub enum IntegrationsAction {
    Load,
    Create,
    Delete(i64),
    Kind(IntegrationKind),
    Dismiss,
}

/// What a request answered.
#[derive(Debug)]
pub enum IntegrationsReply {
    Loaded(Result<Vec<Integration>>),
    Created(Result<Created>),
    Done(Result<()>),
}

impl IntegrationsDialog {
    pub fn act(&mut self, net: &Net, client: &Client, action: IntegrationsAction) {
        match action {
            IntegrationsAction::Kind(kind) => {
                self.kind = kind;
                return;
            }
            IntegrationsAction::Dismiss => {
                self.created = None;
                return;
            }
            _ => {}
        }
        if self.pending {
            return;
        }
        self.pending = true;
        self.error = None;
        let client = client.clone();
        let channel = self.channel;
        match action {
            IntegrationsAction::Load => net.run(async move {
                Reply::Integrations(IntegrationsReply::Loaded(
                    client.integrations(channel).await,
                ))
            }),
            IntegrationsAction::Create => {
                let req = CreateIntegrationRequest {
                    kind: self.kind,
                    name: self.name.trim().to_owned(),
                };
                net.run(async move {
                    let result = client
                        .create_integration(channel, &req)
                        .await
                        .map(|c| Created {
                            url: client.hook_url(&c),
                            name: c.integration.name,
                            kind: c.integration.kind,
                            secret: c.secret,
                        });
                    Reply::Integrations(IntegrationsReply::Created(result))
                });
            }
            IntegrationsAction::Delete(id) => net.run(async move {
                Reply::Integrations(IntegrationsReply::Done(client.delete_integration(id).await))
            }),
            IntegrationsAction::Kind(_) | IntegrationsAction::Dismiss => {}
        }
    }

    /// Fold an answer in. Returns `true` when the list should reload.
    pub fn answer(&mut self, reply: IntegrationsReply) -> bool {
        self.pending = false;
        match reply {
            IntegrationsReply::Loaded(Ok(list)) => {
                self.list = Some(list);
                false
            }
            IntegrationsReply::Created(Ok(created)) => {
                self.name.clear();
                self.created = Some(created);
                true
            }
            IntegrationsReply::Done(Ok(())) => true,
            IntegrationsReply::Loaded(Err(e))
            | IntegrationsReply::Created(Err(e))
            | IntegrationsReply::Done(Err(e)) => {
                self.error = Some(e.to_string());
                false
            }
        }
    }
}

pub fn kind_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Hito => "Hito",
        IntegrationKind::Webhook => tr("Webhook"),
    }
}

pub fn show(ui: &mut Ui, look: &Look, dialog: &mut IntegrationsDialog, actions: &mut Vec<Action>) {
    let p = &look.palette;
    let mut wanted: Vec<IntegrationsAction> = Vec::new();
    note(
        ui,
        look,
        tr("Other apps can post notices here: a Hito project, a build server, a script. Notices are not end-to-end encrypted; the server can read them."),
    );
    if let Some(error) = &dialog.error {
        status_line(ui, look, error, true);
        ui.add_space(8.0);
    }
    if let Some(created) = dialog.created.clone() {
        cr::island(look).inner_margin(12).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(trf("{} is ready", &[&created.name]))
                    .strong()
                    .color(color(p.text_primary)),
            );
            let how = match created.kind {
                IntegrationKind::Hito => tr("In Hito, open the project's settings, add an Ágora connection and paste this address and key. The key is shown only now."),
                IntegrationKind::Webhook => tr("Send a POST to this address with a JSON body whose text field holds the notice. Sign it with the key: the header X-Agora-Signature is sha256= and the HMAC-SHA256 of the body in hex. The key is shown only now."),
            };
            note(ui, look, how);
            copy_row(ui, look, tr("Address"), &created.url, actions);
            copy_row(ui, look, tr("Key"), &created.secret, actions);
            ui.add_space(6.0);
            if cr::primary_button(ui, look, tr("Done"), true).clicked() {
                wanted.push(IntegrationsAction::Dismiss);
            }
        });
        ui.add_space(12.0);
    }
    match &dialog.list {
        None => {
            ui.spinner();
        }
        Some(list) if list.is_empty() => {
            note(ui, look, tr("No integrations in this channel yet."));
        }
        Some(list) => {
            for integration in list {
                cr::island(look).inner_margin(10).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        crate::widgets::icon(ui, "link", 18.0, color(p.text_secondary));
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&integration.name)
                                    .strong()
                                    .color(color(p.text_primary)),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    kind_label(integration.kind),
                                    trf("added {}", &[&crate::view::date(integration.created_at)])
                                ))
                                .size(font_size::CAPTION)
                                .color(color(p.text_tertiary)),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if dialog.admin
                                && cr::secondary_button(ui, look, tr("Remove"), !dialog.pending)
                                    .clicked()
                            {
                                wanted.push(IntegrationsAction::Delete(integration.id));
                            }
                        });
                    });
                });
                ui.add_space(6.0);
            }
        }
    }
    if dialog.admin {
        ui.add_space(10.0);
        ui.label(
            RichText::new(tr("Add an integration"))
                .strong()
                .color(color(p.text_primary)),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for kind in [IntegrationKind::Hito, IntegrationKind::Webhook] {
                if ui
                    .selectable_label(dialog.kind == kind, kind_label(kind))
                    .clicked()
                {
                    wanted.push(IntegrationsAction::Kind(kind));
                }
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(tr("Name"))
                .size(font_size::SMALL)
                .color(color(p.text_secondary)),
        );
        let response = cr::field_frame(look)
            .inner_margin(egui::Margin::symmetric(8, 0))
            .show(ui, |ui| {
                ui.add(
                    TextEdit::singleline(&mut dialog.name)
                        .frame(egui::Frame::NONE)
                        .align(egui::Align2::LEFT_CENTER)
                        .char_limit(60)
                        .hint_text(match dialog.kind {
                            IntegrationKind::Hito => tr("Website launch"),
                            IntegrationKind::Webhook => tr("Build server"),
                        })
                        .desired_width(ui.available_width()),
                )
            })
            .inner;
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let ready = !dialog.name.trim().is_empty() && !dialog.pending;
        ui.add_space(8.0);
        let row = vec2(ui.available_width(), 32.0);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            if cr::primary_button(ui, look, tr("Add"), ready).clicked() || (enter && ready) {
                wanted.push(IntegrationsAction::Create);
            }
        });
    } else {
        ui.add_space(6.0);
        note(ui, look, tr("Workspace admins can add integrations."));
    }
    if dialog.pending {
        ui.spinner();
    }
    actions.extend(wanted.into_iter().map(Action::Integrations));
}

fn copy_row(ui: &mut Ui, look: &Look, label: &str, value: &str, actions: &mut Vec<Action>) {
    ui.label(
        RichText::new(label)
            .size(font_size::CAPTION)
            .color(color(look.palette.text_secondary)),
    );
    ui.horizontal(|ui| {
        if ui.small_button(tr("Copy")).clicked() {
            actions.push(Action::Copy(value.to_owned()));
        }
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .monospace()
                    .size(font_size::CAPTION)
                    .color(color(look.palette.text_primary)),
            )
            .wrap(),
        );
    });
}

fn note(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(
        RichText::new(text)
            .size(font_size::SMALL)
            .color(color(look.palette.text_secondary)),
    );
    ui.add_space(6.0);
}
