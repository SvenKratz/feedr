//! Compact artifact identity and actions; the document gets the visual emphasis.
use super::{BLUE, INK, LINE, MUTED};
use crate::model::{Post, Resource, ResourceKind};
use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Close,
    Expand,
    External,
    Folder,
    Copy,
    Check,
    Plus,
    Minus,
}

#[derive(Default)]
pub(super) struct Actions {
    pub close: bool,
    pub expand: bool,
    pub open: bool,
    pub reveal: bool,
    pub attachment: Option<usize>,
}

pub(super) struct Header<'a> {
    pub post: &'a Post,
    pub resource: &'a Resource,
    pub index: usize,
    pub accent: Color32,
    pub render_markdown: &'a mut bool,
    pub source: Option<&'a str>,
}

impl Header<'_> {
    pub fn show(self, ui: &mut egui::Ui) -> Actions {
        let mut actions = Actions::default();
        let r = self.resource;
        let media = matches!(r.file_type(), "Image" | "Video");
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4., 3.);
            ui.spacing_mut().button_padding = Vec2::new(6., 3.);
            ui.horizontal(|ui| {
                let controls = if media { 56. } else { 26. };
                let width = (ui.available_width() - controls - 4.).max(32.);
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 26.),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(width);
                        if self.post.resources.len() > 1 {
                            // A file switcher, without a second box competing with the toolbar.
                            let visuals = &mut ui.style_mut().visuals;
                            visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
                            visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                            visuals.widgets.inactive.bg_stroke = Stroke::NONE;
                            let mut index = self.index;
                            let combo = egui::ComboBox::from_id_salt("viewer-attachment")
                                .width(width)
                                .selected_text(RichText::new(&r.name).size(15.).strong().color(INK))
                                .truncate()
                                .show_ui(ui, |ui| {
                                    for (i, resource) in self.post.resources.iter().enumerate() {
                                        ui.selectable_value(&mut index, i, &resource.name);
                                    }
                                });
                            combo.response.on_hover_text(format!(
                                "{}\nAttachment {} of {}\n{}",
                                r.name,
                                self.index + 1,
                                self.post.resources.len(),
                                self.post.title
                            ));
                            if index != self.index {
                                actions.attachment = Some(index);
                            }
                        } else {
                            ui.add(
                                egui::Label::new(RichText::new(&r.name).size(15.).strong())
                                    .truncate(),
                            )
                            .on_hover_text(format!("{}\n{}", r.name, self.post.title));
                        }
                    },
                );
                if media {
                    actions.expand = icon_button(ui, Icon::Expand, "Expand preview").clicked();
                }
                actions.close = icon_button(ui, Icon::Close, "Close preview (Esc)").clicked();
            });
            ui.add(
                egui::Label::new(
                    RichText::new(&self.post.context.label)
                        .size(11.)
                        .color(self.accent),
                )
                .truncate(),
            )
            .on_hover_text(format!(
                "{} ({})\n{}",
                self.post.context.label, self.post.context.agent, self.post.title
            ));
            ui.add_space(3.);
            ui.horizontal(|ui| {
                let count = if r.kind == ResourceKind::Url { 2 } else { 3 };
                let width = (ui.available_width() - count as f32 * 30.).max(16.);
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 26.),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_width(width);
                        let text = compact_location(ui, &r.target, width);
                        ui.add(
                            egui::Label::new(RichText::new(text).size(11.).color(MUTED)).truncate(),
                        )
                        .on_hover_text(&r.target);
                    },
                );
                actions.open = icon_button(ui, Icon::External, "Open externally").clicked();
                if r.kind != ResourceKind::Url {
                    actions.reveal = icon_button(ui, Icon::Folder, "Show in folder").clicked();
                }
                copy_button(
                    ui,
                    "Copy location",
                    Some(&r.target),
                    ("location", &r.target),
                );
            });
            if r.file_type() == "Markdown" {
                ui.add_space(3.);
                ui.horizontal(|ui| {
                    egui::Frame::new()
                        .fill(super::PAPER)
                        .corner_radius(4)
                        .inner_margin(2)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 1.;
                            for (label, rendered) in [("Rendered", true), ("Source", false)] {
                                let active = *self.render_markdown == rendered;
                                let response = ui.add_sized(
                                    [64., 23.],
                                    egui::Button::new(
                                        RichText::new(label).size(11.).color(if active {
                                            INK
                                        } else {
                                            MUTED
                                        }),
                                    )
                                    .fill(if active {
                                        Color32::from_rgb(36, 56, 66)
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .stroke(Stroke::NONE)
                                    .corner_radius(3),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::SelectableLabel,
                                        ui.is_enabled(),
                                        active,
                                        label,
                                    )
                                });
                                #[cfg(test)]
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("viewer-control", label)),
                                        response.rect,
                                    )
                                });
                                if response
                                    .on_hover_text(if rendered {
                                        "Render Markdown"
                                    } else {
                                        "Show Markdown source"
                                    })
                                    .clicked()
                                {
                                    *self.render_markdown = rendered;
                                }
                            }
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        copy_button(
                            ui,
                            "Copy Markdown source",
                            self.source,
                            ("source", &r.target),
                        );
                    });
                });
            }
        });
        ui.add_space(7.);
        let rect = ui.available_rect_before_wrap();
        ui.painter()
            .hline(rect.x_range(), rect.top(), Stroke::new(1.0_f32, LINE));
        ui.add_space(10.);
        actions
    }
}

pub(super) fn icon_button(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    let response = ui.add_sized([26., 26.], egui::Button::new("").frame(false));
    let rect = response.rect;
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(("viewer-control", label)), rect));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let active = response.hovered() || response.has_focus();
    if active {
        ui.painter()
            .rect_filled(rect, 4., Color32::from_rgb(34, 50, 61));
    }
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            4.,
            Stroke::new(1.0_f32, BLUE),
            egui::StrokeKind::Inside,
        );
    }
    let color = if !ui.is_enabled() {
        MUTED.gamma_multiply(0.35)
    } else if active {
        INK
    } else {
        MUTED
    };
    paint_icon(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(14.)),
        icon,
        color,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(label)
}

fn copy_button(ui: &mut egui::Ui, label: &str, value: Option<&str>, salt: impl std::hash::Hash) {
    let id = ui.make_persistent_id(salt);
    let time = ui.input(|input| input.time);
    let copied = ui
        .ctx()
        .data(|data| data.get_temp::<f64>(id))
        .is_some_and(|until| until > time);
    if copied {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.add_enabled_ui(value.is_some(), |ui| {
        let response = icon_button(
            ui,
            if copied { Icon::Check } else { Icon::Copy },
            if copied { "Copied" } else { label },
        );
        if response.clicked()
            && let Some(value) = value
        {
            ui.ctx().copy_text(value.to_owned());
            ui.ctx().data_mut(|data| data.insert_temp(id, time + 1.5));
        }
    });
}

fn compact_location(ui: &egui::Ui, text: &str, width: f32) -> String {
    let font = FontId::monospace(11.);
    let fits = |text: &str| {
        ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(text.into(), font.clone(), MUTED)
                .size()
                .x
                <= width
        })
    };
    if fits(text) || text.starts_with("https://") || text.starts_with("http://") {
        return text.to_owned();
    }
    for (index, _) in text.char_indices().filter(|(_, c)| *c == '/') {
        let candidate = format!("…{}", &text[index..]);
        if fits(&candidate) {
            return candidate;
        }
    }
    let chars: Vec<_> = text.chars().collect();
    let mut low = 0;
    let mut high = chars.len();
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let candidate = format!("…{}", chars[chars.len() - mid..].iter().collect::<String>());
        if fits(&candidate) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("…{}", chars[chars.len() - low..].iter().collect::<String>())
}

fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let at = |x, y| {
        Pos2::new(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let stroke = Stroke::new(1.3_f32, color);
    let line = |a, b| {
        p.line_segment([a, b], stroke);
    };
    match icon {
        Icon::Close => {
            line(at(0.22, 0.22), at(0.78, 0.78));
            line(at(0.78, 0.22), at(0.22, 0.78));
        }
        Icon::External => {
            p.add(egui::Shape::line(
                vec![at(0.48, 0.15), at(0.85, 0.15), at(0.85, 0.52)],
                stroke,
            ));
            line(at(0.85, 0.15), at(0.4, 0.6));
            p.add(egui::Shape::line(
                vec![
                    at(0.25, 0.25),
                    at(0.15, 0.25),
                    at(0.15, 0.85),
                    at(0.75, 0.85),
                    at(0.75, 0.75),
                ],
                stroke,
            ));
        }
        Icon::Folder => {
            p.add(egui::Shape::closed_line(
                vec![
                    at(0.08, 0.22),
                    at(0.4, 0.22),
                    at(0.52, 0.36),
                    at(0.92, 0.36),
                    at(0.92, 0.82),
                    at(0.08, 0.82),
                ],
                stroke,
            ));
        }
        Icon::Copy => {
            p.add(egui::Shape::line(
                vec![at(0.62, 0.12), at(0.15, 0.12), at(0.15, 0.63)],
                stroke,
            ));
            p.rect_stroke(
                Rect::from_min_max(at(0.35, 0.32), at(0.85, 0.88)),
                1.,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Check => {
            p.add(egui::Shape::line(
                vec![at(0.15, 0.5), at(0.4, 0.75), at(0.85, 0.25)],
                stroke,
            ));
        }
        Icon::Plus | Icon::Minus => {
            line(at(0.18, 0.5), at(0.82, 0.5));
            if matches!(icon, Icon::Plus) {
                line(at(0.5, 0.18), at(0.5, 0.82));
            }
        }
        Icon::Expand => {
            for (x, y, dx, dy) in [
                (0.12, 0.12, 1., 1.),
                (0.88, 0.12, -1., 1.),
                (0.12, 0.88, 1., -1.),
                (0.88, 0.88, -1., -1.),
            ] {
                p.add(egui::Shape::line(
                    vec![at(x, y + dy * 0.25), at(x, y), at(x + dx * 0.25, y)],
                    stroke,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    fn post() -> Post {
        Post {
            id: "header-test".into(),
            session_id: "session".into(),
            title: "A post title that should not crowd out the artifact".into(),
            body: String::new(),
            resources: vec![
                Resource {
                    original: String::new(),
                    target: "/a/very/long/workspace/日本語/preview-guide.md".into(),
                    kind: ResourceKind::File,
                    name: "preview-guide.md".into(),
                },
                Resource {
                    original: String::new(),
                    target: "/tmp/image.png".into(),
                    kind: ResourceKind::File,
                    name: "image.png".into(),
                },
            ],
            context: Context {
                label: "Build feedr artifact feed | feedr".into(),
                ..Default::default()
            },
            readiness: "ready".into(),
            revision: 1,
            created_at: 0,
            updated_at: 0,
            pinned: false,
            archived: false,
            read: true,
            discovery: None,
        }
    }
    fn frame(
        ctx: &egui::Context,
        post: &Post,
        rendered: &mut bool,
        width: f32,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                time: Some(time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ctx, |ui| {
                        Header {
                            post,
                            resource: &post.resources[0],
                            index: 0,
                            accent: BLUE,
                            render_markdown: rendered,
                            source: Some("# Original Markdown\n\n**Preserve source.**"),
                        }
                        .show(ui);
                        assert!(
                            ui.min_rect().right() <= width + 0.1,
                            "header overflowed at {width}: {:?}",
                            ui.min_rect()
                        );
                        assert!(
                            ui.next_widget_position().y < 140.,
                            "Header should leave the document visible at {width}"
                        );
                    });
            },
        )
    }
    #[test]
    fn long_locations_keep_the_toolbar_on_one_line_at_minimum_and_wide_widths() {
        let post = post();
        for width in [276., 406., 700.] {
            let ctx = egui::Context::default();
            frame(&ctx, &post, &mut true, width, 0., vec![]);
            let controls: Vec<_> = ["Open externally", "Show in folder", "Copy location"]
                .iter()
                .map(|label| {
                    ctx.data(|data| {
                        data.get_temp::<Rect>(egui::Id::new(("viewer-control", label)))
                            .unwrap()
                    })
                })
                .collect();
            assert!(
                controls
                    .windows(2)
                    .all(|pair| (pair[0].center().y - pair[1].center().y).abs() < 0.1
                        && pair[0].right() < pair[1].left())
            );
            assert!(controls.iter().all(|rect| rect.right() <= width + 0.1));
            let close = ctx.data(|data| {
                data.get_temp::<Rect>(egui::Id::new(("viewer-control", "Close preview (Esc)")))
                    .unwrap()
            });
            assert!(
                (close.right() - width).abs() < 0.1,
                "Close must stay at the right edge: {close:?}"
            );
        }
    }
    #[test]
    fn source_mode_and_icon_copy_preserve_raw_markdown() {
        let ctx = egui::Context::default();
        let post = post();
        let mut rendered = true;
        frame(&ctx, &post, &mut rendered, 406., 0., vec![]);
        let pos = ctx.data(|data| {
            data.get_temp::<Rect>(egui::Id::new(("viewer-control", "Source")))
                .unwrap()
                .center()
        });
        let click = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        frame(&ctx, &post, &mut rendered, 406., 0.1, click(pos, true));
        frame(&ctx, &post, &mut rendered, 406., 0.2, click(pos, false));
        assert!(!rendered);
        let pos = ctx.data(|data| {
            data.get_temp::<Rect>(egui::Id::new(("viewer-control", "Copy Markdown source")))
                .unwrap()
                .center()
        });
        frame(&ctx, &post, &mut rendered, 406., 0.3, click(pos, true));
        let output = frame(&ctx, &post, &mut rendered, 406., 0.4, click(pos, false));
        assert!(output.platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::CopyText(text) if text=="# Original Markdown\n\n**Preserve source.**")));
        assert!(!rendered);
    }
}
