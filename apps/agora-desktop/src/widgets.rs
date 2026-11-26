//! Small pieces drawn with the NDL palette: icons, icon buttons, avatars, badges, the message
//! text renderer.

use agora_core::format::{self, Block, Inline};
use egui::{CornerRadius, Frame, Margin, RichText, Stroke, Ui};
use nodalia_theme::creator::Look;
use nodalia_theme::font_size;
use nodalia_theme::for_egui::color;

// The pieces every Enterprise app draws the same way come from the suite's module.
pub use nodalia_theme::enterprise::{avatar, badge, danger_button, icon, icon_button};

/// A file in a message: its kind, name and size, and a button to save it. True when the person
/// asked to save it.
pub fn file_card(ui: &mut Ui, look: &Look, attachment: &agora_core::content::Attachment) -> bool {
    let p = &look.palette;
    let mut save = false;
    Frame::NONE
        .fill(color(p.surface_sunken))
        .stroke(Stroke::new(1.0, color(p.border_subtle)))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_max_width(360.0);
            ui.horizontal(|ui| {
                let kind = if attachment.mime.starts_with("image/") {
                    "image"
                } else {
                    "file"
                };
                icon(ui, kind, 28.0, color(p.accent_text));
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.set_max_width(250.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(&attachment.name)
                                .strong()
                                .color(color(p.text_primary)),
                        )
                        .truncate(),
                    );
                    ui.label(
                        RichText::new(agora_core::content::human_size(attachment.size))
                            .size(font_size::CAPTION)
                            .color(color(p.text_secondary)),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save = icon_button(ui, look, "save", crate::i18n::tr("Save"), false).clicked();
                });
            });
        });
    save
}

/// Render a message body: paragraphs, quotes and code blocks, with bold, italic, strike,
/// code spans, links and mentions. Nothing in the body is ever interpreted as markup beyond
/// that, and links only open on a click.
pub fn message_body(ui: &mut Ui, look: &Look, body: &str, me: &str) {
    let p = &look.palette;
    for block in format::parse(body) {
        match block {
            Block::Paragraph(spans) => inline_row(ui, look, &spans, me),
            Block::Quote(spans) => {
                Frame::NONE
                    .inner_margin(Margin {
                        left: 10,
                        right: 0,
                        top: 2,
                        bottom: 2,
                    })
                    .show(ui, |ui| {
                        let top = ui.cursor().min;
                        inline_row(ui, look, &spans, me);
                        let bottom = ui.min_rect().bottom();
                        ui.painter().line_segment(
                            [
                                egui::pos2(top.x - 8.0, top.y),
                                egui::pos2(top.x - 8.0, bottom),
                            ],
                            Stroke::new(3.0, color(p.border_strong)),
                        );
                    });
            }
            Block::Code { code, .. } => {
                Frame::NONE
                    .fill(color(p.surface_sunken))
                    .stroke(Stroke::new(1.0, color(p.border_subtle)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(RichText::new(code).monospace().color(color(p.text_primary)));
                    });
            }
        }
    }
}

fn inline_row(ui: &mut Ui, look: &Look, spans: &[Inline], me: &str) {
    if !spans.iter().any(|s| matches!(s, Inline::Link(_))) {
        // One galley for the paragraph, so it wraps like text rather than like a row of labels.
        ui.label(paragraph(ui, look, spans, me));
        return;
    }
    let p = &look.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for span in spans {
            match span {
                Inline::Text(text) => {
                    let mut lines = text.split('\n').peekable();
                    while let Some(line) = lines.next() {
                        if !line.is_empty() {
                            ui.label(RichText::new(line).color(color(p.text_primary)));
                        }
                        if lines.peek().is_some() {
                            ui.end_row();
                        }
                    }
                }
                Inline::Bold(t) => {
                    ui.label(RichText::new(t).strong().color(color(p.text_primary)));
                }
                Inline::Italic(t) => {
                    ui.label(RichText::new(t).italics().color(color(p.text_primary)));
                }
                Inline::Strike(t) => {
                    ui.label(
                        RichText::new(t)
                            .strikethrough()
                            .color(color(p.text_secondary)),
                    );
                }
                Inline::Code(t) => {
                    ui.label(
                        RichText::new(t)
                            .monospace()
                            .background_color(color(p.surface_sunken))
                            .color(color(p.text_primary)),
                    );
                }
                Inline::Link(url) => {
                    ui.hyperlink_to(RichText::new(url).color(color(p.text_link)), url);
                }
                Inline::Mention(name) => {
                    let ink = if name == me {
                        color(p.text_on_accent)
                    } else {
                        color(p.accent_text)
                    };
                    let fill = if name == me {
                        color(p.accent_default)
                    } else {
                        color(p.accent_subtle)
                    };
                    ui.label(
                        RichText::new(format!("@{name}"))
                            .strong()
                            .color(ink)
                            .background_color(fill),
                    );
                }
            }
        }
    });
}

/// A paragraph without links as one layout job.
fn paragraph(ui: &Ui, look: &Look, spans: &[Inline], me: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let p = &look.palette;
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat::simple(body.clone(), color(p.text_primary));
    let mut job = LayoutJob::default();
    for span in spans {
        match span {
            Inline::Text(t) => job.append(t, 0.0, plain.clone()),
            Inline::Bold(t) => {
                job.append(
                    t,
                    0.0,
                    TextFormat {
                        color: ui.visuals().strong_text_color(),
                        ..plain.clone()
                    },
                );
            }
            Inline::Italic(t) => job.append(
                t,
                0.0,
                TextFormat {
                    italics: true,
                    ..plain.clone()
                },
            ),
            Inline::Strike(t) => job.append(
                t,
                0.0,
                TextFormat {
                    color: color(p.text_secondary),
                    strikethrough: Stroke::new(1.0, color(p.text_secondary)),
                    ..plain.clone()
                },
            ),
            Inline::Code(t) => job.append(
                t,
                0.0,
                TextFormat {
                    font_id: mono.clone(),
                    background: color(p.surface_sunken),
                    ..plain.clone()
                },
            ),
            Inline::Mention(name) => {
                let mine = name == me;
                let ink = if mine {
                    color(p.text_on_accent)
                } else {
                    color(p.accent_text)
                };
                let fill = if mine {
                    color(p.accent_default)
                } else {
                    color(p.accent_subtle)
                };
                job.append(
                    &format!("@{name}"),
                    0.0,
                    TextFormat {
                        color: ink,
                        background: fill,
                        ..plain.clone()
                    },
                );
            }
            Inline::Link(url) => job.append(
                url,
                0.0,
                TextFormat {
                    color: color(p.text_link),
                    ..plain.clone()
                },
            ),
        }
    }
    job.wrap.max_width = ui.available_width();
    job
}

/// Message ticks centred at `at`: one for sent, two for delivered, two in the accent for read.
pub fn ticks(
    painter: &egui::Painter,
    look: &Look,
    at: egui::Pos2,
    delivery: crate::model::Delivery,
) {
    use crate::model::Delivery;
    let p = &look.palette;
    let colour = match delivery {
        Delivery::Read => color(p.accent_text),
        _ => color(p.text_tertiary),
    };
    let stroke = egui::Stroke::new(1.6, colour);
    let check = |dx: f32| {
        let o = at + egui::vec2(dx, 0.0);
        painter.line_segment(
            [o + egui::vec2(-4.5, 0.0), o + egui::vec2(-1.5, 3.0)],
            stroke,
        );
        painter.line_segment(
            [o + egui::vec2(-1.5, 3.0), o + egui::vec2(4.5, -3.5)],
            stroke,
        );
    };
    match delivery {
        Delivery::Sent => check(0.0),
        Delivery::Delivered | Delivery::Read => {
            check(-2.5);
            check(2.5);
        }
    }
}
