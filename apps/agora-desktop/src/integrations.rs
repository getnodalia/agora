//! Integrations: other apps (Hito, a script, a build server) that post notices to a channel
//! through a signed web address. Admins add and remove them; everyone in the channel sees them.
//!
//! Adding one is a short guided flow: choose what to connect, name it, then follow the steps
//! that finish the job in the other app and send a test notice to see it arrive. Requests run on
//! the network runtime and come back as [`Reply::Integrations`]; this module only keeps the
//! dialog's state, draws it and starts requests.

use std::collections::HashMap;

use agora_client::Client;
use agora_core::ChannelId;
use agora_protocol::{
    CreateIntegrationRequest, Integration, IntegrationKind, IntegrationNotice, Timestamp,
};
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
    pub id: i64,
    pub name: String,
    pub kind: IntegrationKind,
    pub url: String,
    pub secret: String,
}

/// Where the person is in the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// What is connected now.
    List,
    /// What to connect.
    Choose,
    /// What to call it.
    Name,
}

#[derive(Clone, Debug)]
pub struct IntegrationsDialog {
    pub channel: ChannelId,
    pub channel_name: String,
    /// Whether this person can add and remove integrations here.
    pub admin: bool,
    pub list: Option<Vec<Integration>>,
    /// When each integration last posted a notice.
    pub last_notice: HashMap<i64, Timestamp>,
    pub step: Step,
    pub kind: IntegrationKind,
    pub name: String,
    pub created: Option<Created>,
    /// A test notice was sent for this integration.
    pub tested: Option<i64>,
    pub pending: bool,
    pub error: Option<String>,
}

impl IntegrationsDialog {
    pub fn new(channel: ChannelId, admin: bool, channel_name: &str) -> IntegrationsDialog {
        IntegrationsDialog {
            channel,
            channel_name: channel_name.to_owned(),
            admin,
            list: None,
            last_notice: HashMap::new(),
            step: Step::List,
            kind: IntegrationKind::Hito,
            name: String::new(),
            created: None,
            tested: None,
            pending: false,
            error: None,
        }
    }

    /// The name Hito's connection gets when the person does not choose one.
    fn hito_name(&self) -> String {
        trf("Hito in #{}", &[&self.channel_name])
    }
}

/// What the dialog asked for.
#[derive(Clone, Debug)]
pub enum IntegrationsAction {
    Load,
    Create,
    /// Connect Hito in one go, with the default name.
    ConnectHito,
    Delete(i64),
    Test(i64),
    /// Choose what to connect and move on to its name.
    Kind(IntegrationKind),
    Step(Step),
    Dismiss,
}

/// What a request answered.
#[derive(Debug)]
pub enum IntegrationsReply {
    Loaded(Result<(Vec<Integration>, Vec<IntegrationNotice>)>),
    Created(Result<Created>),
    Tested(i64, Result<()>),
    Done(Result<()>),
}

impl IntegrationsDialog {
    pub fn act(&mut self, net: &Net, client: &Client, action: IntegrationsAction) {
        match action {
            IntegrationsAction::Kind(kind) => {
                self.kind = kind;
                self.name = match kind {
                    IntegrationKind::Hito => self.hito_name(),
                    IntegrationKind::Webhook => String::new(),
                };
                self.step = Step::Name;
                self.error = None;
                return;
            }
            IntegrationsAction::Step(step) => {
                self.step = step;
                self.error = None;
                return;
            }
            IntegrationsAction::Dismiss => {
                self.created = None;
                self.tested = None;
                self.step = Step::List;
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
                let result = async {
                    let list = client.integrations(channel).await?;
                    let notices = client.notices(channel).await?;
                    Ok((list, notices))
                }
                .await;
                Reply::Integrations(IntegrationsReply::Loaded(result))
            }),
            IntegrationsAction::Create | IntegrationsAction::ConnectHito => {
                if matches!(action, IntegrationsAction::ConnectHito) {
                    self.kind = IntegrationKind::Hito;
                    self.name = self.hito_name();
                }
                let req = CreateIntegrationRequest {
                    kind: self.kind,
                    name: self.name.trim().to_owned(),
                };
                net.run(async move {
                    let result = client
                        .create_integration(channel, &req)
                        .await
                        .map(|c| Created {
                            id: c.integration.id,
                            url: client.hook_url(&c),
                            name: c.integration.name,
                            kind: c.integration.kind,
                            secret: c.secret,
                        });
                    Reply::Integrations(IntegrationsReply::Created(result))
                });
            }
            IntegrationsAction::Test(id) => net.run(async move {
                Reply::Integrations(IntegrationsReply::Tested(
                    id,
                    client.test_integration(id).await,
                ))
            }),
            IntegrationsAction::Delete(id) => net.run(async move {
                Reply::Integrations(IntegrationsReply::Done(client.delete_integration(id).await))
            }),
            IntegrationsAction::Kind(_)
            | IntegrationsAction::Step(_)
            | IntegrationsAction::Dismiss => {}
        }
    }

    /// Fold an answer in. Returns `true` when the list should reload.
    pub fn answer(&mut self, reply: IntegrationsReply) -> bool {
        self.pending = false;
        match reply {
            IntegrationsReply::Loaded(Ok((list, notices))) => {
                self.last_notice.clear();
                for notice in &notices {
                    let slot = self.last_notice.entry(notice.integration_id).or_insert(0);
                    *slot = (*slot).max(notice.created_at);
                }
                self.list = Some(list);
                false
            }
            IntegrationsReply::Created(Ok(created)) => {
                self.name.clear();
                self.step = Step::List;
                self.created = Some(created);
                true
            }
            IntegrationsReply::Tested(id, Ok(())) => {
                self.tested = Some(id);
                true
            }
            IntegrationsReply::Done(Ok(())) => true,
            IntegrationsReply::Loaded(Err(e))
            | IntegrationsReply::Created(Err(e))
            | IntegrationsReply::Tested(_, Err(e))
            | IntegrationsReply::Done(Err(e)) => {
                self.error = Some(crate::i18n::explain(&e));
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
    let mut wanted: Vec<IntegrationsAction> = Vec::new();
    if let Some(error) = &dialog.error {
        status_line(ui, look, error, true);
        ui.add_space(8.0);
    }
    if let Some(created) = dialog.created.clone() {
        created_view(ui, look, dialog, &created, &mut wanted, actions);
    } else {
        match dialog.step {
            Step::List => list_view(ui, look, dialog, &mut wanted),
            Step::Choose => choose_view(ui, look, dialog, &mut wanted),
            Step::Name => name_view(ui, look, dialog, &mut wanted),
        }
    }
    if dialog.pending {
        ui.add_space(6.0);
        ui.spinner();
    }
    actions.extend(wanted.into_iter().map(Action::Integrations));
}

/// A card on the dialog's surface: one step darker than the dialog, with a hairline.
fn card(look: &Look) -> egui::Frame {
    let p = &look.palette;
    egui::Frame::NONE
        .fill(color(p.surface_window))
        .stroke(egui::Stroke::new(1.0, color(p.border_subtle)))
        .corner_radius(egui::CornerRadius::same(10))
}

fn heading(ui: &mut Ui, look: &Look, text: &str) {
    ui.label(
        RichText::new(text)
            .size(font_size::LARGE)
            .strong()
            .color(color(look.palette.text_primary)),
    );
    ui.add_space(4.0);
}

/// "Step n of 3" and a thin track, so the person knows how far the flow goes.
fn progress(ui: &mut Ui, look: &Look, step: usize, label: &str) {
    ui.label(
        RichText::new(trf("Step {} of 3 · {}", &[&step.to_string(), label]))
            .size(font_size::CAPTION)
            .color(color(look.palette.accent_default)),
    );
    ui.add_space(2.0);
}

fn list_view(
    ui: &mut Ui,
    look: &Look,
    dialog: &IntegrationsDialog,
    wanted: &mut Vec<IntegrationsAction>,
) {
    let p = &look.palette;
    note(
        ui,
        look,
        tr("Connect another app to this channel and its notices appear here as they happen: a Hito project, a build server, a script."),
    );
    match &dialog.list {
        None => {
            ui.spinner();
        }
        Some(list) if list.is_empty() => {
            card(look).inner_margin(14).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new(tr("Nothing is connected to this channel yet."))
                        .strong()
                        .color(color(p.text_primary)),
                );
                note(
                    ui,
                    look,
                    tr("Notices from other apps are not end-to-end encrypted: the server can read them."),
                );
            });
        }
        Some(list) => {
            for integration in list {
                integration_card(ui, look, dialog, integration, wanted);
                ui.add_space(6.0);
            }
            note(
                ui,
                look,
                tr("Notices from other apps are not end-to-end encrypted: the server can read them."),
            );
        }
    }
    if dialog.admin {
        ui.add_space(8.0);
        let row = vec2(ui.available_width(), 32.0);
        ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
            if cr::primary_button(ui, look, tr("Connect an app"), !dialog.pending).clicked() {
                wanted.push(IntegrationsAction::Step(Step::Choose));
            }
        });
    } else {
        ui.add_space(6.0);
        note(ui, look, tr("Workspace admins can connect apps."));
    }
}

fn integration_card(
    ui: &mut Ui,
    look: &Look,
    dialog: &IntegrationsDialog,
    integration: &Integration,
    wanted: &mut Vec<IntegrationsAction>,
) {
    let p = &look.palette;
    card(look).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            crate::widgets::icon(ui, "link", 18.0, color(p.text_secondary));
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(&integration.name)
                        .strong()
                        .color(color(p.text_primary)),
                );
                let last = match dialog.last_notice.get(&integration.id) {
                    Some(at) => trf("last notice {}", &[&crate::view::date(*at)]),
                    None => tr("no notices yet").to_owned(),
                };
                ui.label(
                    RichText::new(format!("{} · {}", kind_label(integration.kind), last))
                        .size(font_size::CAPTION)
                        .color(color(p.text_tertiary)),
                );
            });
        });
        if dialog.admin {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if cr::secondary_button(ui, look, tr("Send a test"), !dialog.pending).clicked() {
                    wanted.push(IntegrationsAction::Test(integration.id));
                }
                if cr::secondary_button(ui, look, tr("Remove"), !dialog.pending).clicked() {
                    wanted.push(IntegrationsAction::Delete(integration.id));
                }
            });
            if dialog.tested == Some(integration.id) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(tr("Test notice sent. Look for it in the channel."))
                        .size(font_size::CAPTION)
                        .color(color(p.status_success_fg)),
                );
            }
        }
    });
}

fn choose_view(
    ui: &mut Ui,
    look: &Look,
    dialog: &IntegrationsDialog,
    wanted: &mut Vec<IntegrationsAction>,
) {
    progress(ui, look, 1, tr("What to connect"));
    heading(ui, look, tr("What do you want to connect?"));
    choice_card(
        ui,
        look,
        "Hito",
        tr("Tasks and milestones of a Hito project show up here as they are completed or change."),
        tr("Connect Hito"),
        !dialog.pending,
        true,
        || wanted.push(IntegrationsAction::ConnectHito),
    );
    ui.add_space(8.0);
    choice_card(
        ui,
        look,
        tr("Another app or script"),
        tr("Anything that can send a signed web request: a build server, monitoring, or your own script."),
        tr("Set up"),
        !dialog.pending,
        false,
        || wanted.push(IntegrationsAction::Kind(IntegrationKind::Webhook)),
    );
    ui.add_space(10.0);
    if cr::secondary_button(ui, look, tr("Back"), true).clicked() {
        wanted.push(IntegrationsAction::Step(Step::List));
    }
}

#[allow(clippy::too_many_arguments)]
fn choice_card(
    ui: &mut Ui,
    look: &Look,
    title: &str,
    body: &str,
    button: &str,
    enabled: bool,
    primary: bool,
    on_click: impl FnOnce(),
) {
    let p = &look.palette;
    card(look).inner_margin(14).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(title)
                .size(font_size::LARGE)
                .strong()
                .color(color(p.text_primary)),
        );
        note(ui, look, body);
        let row = vec2(ui.available_width(), 32.0);
        let clicked = ui
            .allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
                if primary {
                    cr::primary_button(ui, look, button, enabled).clicked()
                } else {
                    cr::secondary_button(ui, look, button, enabled).clicked()
                }
            })
            .inner;
        if clicked {
            on_click();
        }
    });
}

fn name_view(
    ui: &mut Ui,
    look: &Look,
    dialog: &mut IntegrationsDialog,
    wanted: &mut Vec<IntegrationsAction>,
) {
    let p = &look.palette;
    progress(ui, look, 2, tr("Name it"));
    heading(ui, look, &trf("Connect {}", &[kind_label(dialog.kind)]));
    note(
        ui,
        look,
        tr("The name appears on every notice, so people know where it came from."),
    );
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
                    .desired_width(ui.available_width()),
            )
        })
        .inner;
    if !response.has_focus() && ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let ready = !dialog.name.trim().is_empty() && !dialog.pending;
    ui.add_space(12.0);
    let row = vec2(ui.available_width(), 32.0);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        if cr::primary_button(ui, look, tr("Create connection"), ready).clicked()
            || (enter && ready)
        {
            wanted.push(IntegrationsAction::Create);
        }
        if cr::secondary_button(ui, look, tr("Back"), true).clicked() {
            wanted.push(IntegrationsAction::Step(Step::Choose));
        }
    });
}

fn created_view(
    ui: &mut Ui,
    look: &Look,
    dialog: &IntegrationsDialog,
    created: &Created,
    wanted: &mut Vec<IntegrationsAction>,
    actions: &mut Vec<Action>,
) {
    let p = &look.palette;
    progress(ui, look, 3, tr("Finish in the other app"));
    heading(ui, look, &trf("Connection created: {}", &[&created.name]));
    note(
        ui,
        look,
        tr("The key is shown only now. If you lose it, remove the connection and make a new one."),
    );
    match created.kind {
        IntegrationKind::Hito => {
            numbered(
                ui,
                look,
                1,
                tr("In Hito, open the project and go to its settings, then Integrations."),
            );
            numbered(ui, look, 2, tr("Choose Ágora and paste this address:"));
            copy_row(ui, look, tr("Address"), &created.url, actions);
            numbered(ui, look, 3, tr("Paste this key:"));
            copy_row(ui, look, tr("Key"), &created.secret, actions);
        }
        IntegrationKind::Webhook => {
            numbered(ui, look, 1, tr("Send an HTTP POST with a JSON body whose text field holds the notice, to this address:"));
            copy_row(ui, look, tr("Address"), &created.url, actions);
            numbered(ui, look, 2, tr("Sign the body with this key. Send the HMAC-SHA256 in hex as sha256=… in the X-Agora-Signature header:"));
            copy_row(ui, look, tr("Key"), &created.secret, actions);
            numbered(
                ui,
                look,
                3,
                tr("Or start from this command and change the text:"),
            );
            if cr::secondary_button(ui, look, tr("Copy example command"), true).clicked() {
                actions.push(Action::Copy(example_command(created)));
            }
        }
    }
    ui.add_space(10.0);
    card(look).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            RichText::new(tr("Check it works"))
                .strong()
                .color(color(p.text_primary)),
        );
        note(
            ui,
            look,
            tr("Send a test notice to this channel to see it arrive."),
        );
        ui.horizontal(|ui| {
            if cr::secondary_button(ui, look, tr("Send a test"), !dialog.pending).clicked() {
                wanted.push(IntegrationsAction::Test(created.id));
            }
            if dialog.tested == Some(created.id) {
                ui.label(
                    RichText::new(tr("Sent. Look for it in the channel."))
                        .size(font_size::SMALL)
                        .color(color(p.status_success_fg)),
                );
            }
        });
    });
    ui.add_space(10.0);
    let row = vec2(ui.available_width(), 32.0);
    ui.allocate_ui_with_layout(row, Layout::right_to_left(Align::Center), |ui| {
        if cr::primary_button(ui, look, tr("Done"), true).clicked() {
            wanted.push(IntegrationsAction::Dismiss);
        }
    });
}

/// A shell command that posts one signed notice, with the key filled in.
fn example_command(created: &Created) -> String {
    format!(
        "body='{{\"text\":\"Hello from the command line\"}}'\n\
         sig=$(printf %s \"$body\" | openssl dgst -sha256 -hmac '{key}' -hex | sed 's/^.* //')\n\
         curl -X POST '{url}' -H 'Content-Type: application/json' \\\n  \
         -H \"X-Agora-Signature: sha256=$sig\" -d \"$body\"\n",
        key = created.secret,
        url = created.url,
    )
}

/// A step of the instructions: its number in a disc, then the text beside it.
fn numbered(ui: &mut Ui, look: &Look, n: usize, text: &str) {
    let p = &look.palette;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 10.0, color(p.accent_default));
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::proportional(11.0),
            color(p.text_on_accent),
        );
        ui.add(egui::Label::new(RichText::new(text).color(color(p.text_primary))).wrap());
    });
}

fn copy_row(ui: &mut Ui, look: &Look, label: &str, value: &str, actions: &mut Vec<Action>) {
    let p = &look.palette;
    ui.add_space(2.0);
    cr::field_frame(look)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(label)
                    .size(font_size::CAPTION)
                    .color(color(p.text_tertiary)),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(value)
                        .monospace()
                        .size(font_size::CAPTION)
                        .color(color(p.text_primary)),
                )
                .wrap(),
            );
            ui.add_space(2.0);
            if ui.small_button(tr("Copy")).clicked() {
                actions.push(Action::Copy(value.to_owned()));
            }
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
