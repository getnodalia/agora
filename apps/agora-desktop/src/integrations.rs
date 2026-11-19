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

    /// The name a GitHub connection gets until the person types the repository.
    fn github_name(&self) -> String {
        trf("GitHub in #{}", &[&self.channel_name])
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
    /// A ready-made web connection (builds, monitoring, a script) with this default name.
    Preset(&'static str),
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
                    IntegrationKind::GitHub => self.github_name(),
                    IntegrationKind::Webhook => String::new(),
                };
                self.step = Step::Name;
                self.error = None;
                return;
            }
            IntegrationsAction::Preset(name) => {
                self.kind = IntegrationKind::Webhook;
                self.name = tr(name).to_owned();
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
            | IntegrationsAction::Preset(_)
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

/// The icon that stands for an integration, in the list and on its notices.
pub fn kind_icon(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Hito => "check-square",
        IntegrationKind::GitHub => "branch",
        IntegrationKind::Webhook => "link",
    }
}

pub fn kind_label(kind: IntegrationKind) -> &'static str {
    match kind {
        IntegrationKind::Hito => "Hito",
        IntegrationKind::GitHub => "GitHub",
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
        if dialog.admin && matches!(dialog.step, Step::List | Step::Choose) {
            tab_bar(ui, look, dialog, &mut wanted);
        }
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

/// What is connected, and the gallery of plugins to add.
fn tab_bar(
    ui: &mut Ui,
    look: &Look,
    dialog: &IntegrationsDialog,
    wanted: &mut Vec<IntegrationsAction>,
) {
    let connected = match &dialog.list {
        Some(list) => trf("Connected ({})", &[&list.len().to_string()]),
        None => tr("Connected").to_owned(),
    };
    ui.horizontal(|ui| {
        if cr::tab(ui, look, &connected, dialog.step == Step::List).clicked() {
            wanted.push(IntegrationsAction::Step(Step::List));
        }
        if cr::tab(ui, look, tr("Plugins"), dialog.step == Step::Choose).clicked() {
            wanted.push(IntegrationsAction::Step(Step::Choose));
        }
    });
    ui.add_space(8.0);
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
            crate::widgets::icon(
                ui,
                kind_icon(integration.kind),
                18.0,
                color(p.text_secondary),
            );
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
    note(
        ui,
        look,
        tr("Pick what to connect to this channel. Each one posts its notices here."),
    );
    let hito_connected = dialog
        .list
        .as_ref()
        .is_some_and(|list| list.iter().any(|i| i.kind == IntegrationKind::Hito));
    let connected = tr("Connected");
    let enabled = !dialog.pending;
    let tiles: [(&str, &str, &str, Option<&str>, IntegrationsAction); 6] = [
        (
            "check-square",
            "Hito",
            tr("Tasks and milestones of a project, as they change."),
            hito_connected.then_some(connected),
            IntegrationsAction::ConnectHito,
        ),
        (
            "branch",
            "GitHub",
            tr("Pushes, pull requests, issues and releases of a repository."),
            None,
            IntegrationsAction::Kind(IntegrationKind::GitHub),
        ),
        (
            "package",
            tr("Build server"),
            tr("Builds and deployments: started, passed, failed."),
            None,
            IntegrationsAction::Preset("Builds"),
        ),
        (
            "monitor",
            tr("Monitoring"),
            tr("Alerts from your servers and services."),
            None,
            IntegrationsAction::Preset("Monitoring"),
        ),
        (
            "terminal",
            tr("Script"),
            tr("Anything you run yourself, from a cron job to a shell."),
            None,
            IntegrationsAction::Preset("My script"),
        ),
        (
            "link",
            tr("Another app"),
            tr("Any app that can send a signed web request."),
            None,
            IntegrationsAction::Kind(IntegrationKind::Webhook),
        ),
    ];
    let gap = 10.0;
    let width = ((ui.available_width() - gap) / 2.0).floor();
    for pair in tiles.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (icon, title, blurb, status, action) in pair {
                if plugin_tile(ui, look, width, icon, title, blurb, *status, enabled) {
                    wanted.push(action.clone());
                }
            }
        });
        ui.add_space(gap - 4.0);
    }
}

/// One plugin in the gallery: its icon, name and a line about it. The whole tile is the button.
#[allow(clippy::too_many_arguments)]
fn plugin_tile(
    ui: &mut Ui,
    look: &Look,
    width: f32,
    icon: &str,
    title: &str,
    blurb: &str,
    status: Option<&str>,
    enabled: bool,
) -> bool {
    use nodalia_theme::icons;
    let p = &look.palette;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 112.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, title));
    let hot = enabled && response.hovered();
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        egui::CornerRadius::same(12),
        color(p.surface_window),
        egui::Stroke::new(
            if hot || response.has_focus() {
                1.5
            } else {
                1.0
            },
            color(if hot || response.has_focus() {
                p.accent_default
            } else {
                p.border_subtle
            }),
        ),
        egui::StrokeKind::Inside,
    );
    let badge = egui::Rect::from_min_size(rect.min + vec2(12.0, 12.0), vec2(36.0, 36.0));
    painter.rect_filled(badge, egui::CornerRadius::same(10), color(p.accent_subtle));
    if let Some(i) = icons::get(icon) {
        icons::paint(
            &painter,
            badge.shrink(8.0),
            i,
            icons::Style::bold(color(p.accent_text)),
        );
    }
    if let Some(status) = status {
        let galley = painter.layout_no_wrap(
            status.to_owned(),
            egui::FontId::proportional(font_size::CAPTION),
            color(p.status_success_fg),
        );
        painter.galley(
            egui::pos2(rect.max.x - 12.0 - galley.size().x, rect.min.y + 14.0),
            galley,
            color(p.status_success_fg),
        );
    }
    let name = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::new(font_size::LARGE, look.strong.clone()),
        color(p.text_primary),
    );
    painter.galley(rect.min + vec2(12.0, 56.0), name, color(p.text_primary));
    let text = painter.layout(
        blurb.to_owned(),
        egui::FontId::proportional(font_size::CAPTION),
        color(p.text_tertiary),
        width - 24.0,
    );
    painter.galley(rect.min + vec2(12.0, 78.0), text, color(p.text_tertiary));
    if enabled {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    } else {
        false
    }
}

fn name_view(
    ui: &mut Ui,
    look: &Look,
    dialog: &mut IntegrationsDialog,
    wanted: &mut Vec<IntegrationsAction>,
) {
    let p = &look.palette;
    progress(ui, look, 2, tr("Name it"));
    let title = match dialog.kind {
        IntegrationKind::Hito | IntegrationKind::GitHub => {
            trf("Connect {}", &[kind_label(dialog.kind)])
        }
        IntegrationKind::Webhook => tr("Set up a web connection").to_owned(),
    };
    heading(ui, look, &title);
    note(
        ui,
        look,
        if dialog.kind == IntegrationKind::GitHub {
            tr("Name it after the repository, for example acme/app. The name appears on every notice.")
        } else {
            tr("The name appears on every notice, so people know where it came from.")
        },
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
        IntegrationKind::GitHub => {
            numbered(
                ui,
                look,
                1,
                tr("On GitHub, open the repository and go to Settings, Webhooks, then Add webhook."),
            );
            numbered(ui, look, 2, tr("Paste this address as the Payload URL:"));
            copy_row(ui, look, tr("Address"), &created.url, actions);
            numbered(ui, look, 3, tr("Paste this key as the Secret:"));
            copy_row(ui, look, tr("Key"), &created.secret, actions);
            numbered(
                ui,
                look,
                4,
                tr("Leave the content type as it is and choose Send me everything. Ágora shows only what matters."),
            );
            numbered(
                ui,
                look,
                5,
                tr("Press Add webhook. GitHub sends a first message, and Ágora answers here that it is connected."),
            );
            note(
                ui,
                look,
                tr("GitHub has to reach your server, so the address must be public and use https."),
            );
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
