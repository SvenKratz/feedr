//! Resource surfaces are the primary click targets in the feed.
use super::{BLUE, INK, MUTED, Preview, age, load_preview, opens_in_sidebar};
use crate::model::{Post, Resource, ResourceKind};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{
    collections::HashMap,
    fs,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime},
};

const MAX_CACHED: usize = 128;
pub(super) const ROW_HEIGHT: f32 = 42.;
#[derive(Clone, Copy)]
pub(super) struct RowOptions {
    pub width: f32,
    pub selected: bool,
    pub render_markdown: bool,
}
type Stamp = Option<(Option<SystemTime>, u64)>;
struct Job {
    key: String,
    stamp: Stamp,
    post: Post,
    resource: Resource,
    ctx: egui::Context,
}
struct Finished {
    key: String,
    stamp: Stamp,
    preview: Preview,
}
enum Content {
    Loading,
    Text { rendered: String, source: String },
    Image(egui::TextureHandle),
    Unavailable(String),
}
struct Entry {
    stamp: Stamp,
    checked: Instant,
    queued: bool,
    content: Content,
    seen: u64,
}

pub(super) struct Artifacts {
    entries: HashMap<String, Entry>,
    jobs: mpsc::SyncSender<Job>,
    results: mpsc::Receiver<Finished>,
}
impl Artifacts {
    pub fn new() -> Self {
        let (jobs, rx) = mpsc::sync_channel::<Job>(32);
        let (tx, results) = mpsc::sync_channel(4);
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                let preview = load_preview(&job.post, &job.resource, 256);
                if tx
                    .send(Finished {
                        key: job.key,
                        stamp: job.stamp,
                        preview,
                    })
                    .is_err()
                {
                    break;
                }
                job.ctx.request_repaint();
            }
        });
        Self {
            entries: HashMap::new(),
            jobs,
            results,
        }
    }
    pub fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.results.try_recv() {
            let Some(entry) = self.entries.get_mut(&done.key) else {
                continue;
            };
            if entry.stamp != done.stamp {
                continue;
            }
            entry.content = match done.preview {
                Preview::Image(image) => Content::Image(ctx.load_texture(
                    format!("inline:{}:{:?}", done.key, done.stamp),
                    image,
                    egui::TextureOptions::LINEAR,
                )),
                Preview::Text(text) => Content::Text {
                    rendered: excerpt(&text, true),
                    source: excerpt(&text, false),
                },
                Preview::Error(e) | Preview::Unsupported(e) => Content::Unavailable(e),
            };
        }
    }
    fn entry(&mut self, ui: &egui::Ui, post: &Post, r: &Resource) -> &mut Entry {
        let key = format!("{}:{}", post.context.host, r.target);
        let frame = ui.ctx().cumulative_frame_nr();
        let entry = self.entries.entry(key.clone()).or_insert_with(|| Entry {
            stamp: None,
            checked: Instant::now() - Duration::from_secs(2),
            queued: false,
            content: Content::Loading,
            seen: frame,
        });
        entry.seen = frame;
        if entry.checked.elapsed() >= Duration::from_secs(1) {
            let stamp = if post.context.is_local() {
                fs::metadata(&r.target)
                    .ok()
                    .map(|m| (m.modified().ok(), m.len()))
            } else {
                None
            };
            if stamp != entry.stamp {
                entry.stamp = stamp;
                entry.queued = false;
                entry.content = Content::Loading;
            }
            entry.checked = Instant::now();
        }
        if matches!(entry.content, Content::Loading) && !entry.queued {
            entry.queued = self
                .jobs
                .try_send(Job {
                    key: key.clone(),
                    stamp: entry.stamp,
                    post: post.clone(),
                    resource: r.clone(),
                    ctx: ui.ctx().clone(),
                })
                .is_ok();
        }
        while self.entries.len() > MAX_CACHED {
            let oldest = self
                .entries
                .iter()
                .filter(|(k, _)| *k != &key)
                .min_by_key(|(_, e)| e.seen)
                .map(|(k, _)| k.clone());
            if let Some(old) = oldest {
                self.entries.remove(&old);
            } else {
                break;
            }
        }
        self.entries
            .get_mut(&key)
            .expect("Current artifact is retained")
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        p: &Post,
        r: &Resource,
        index: usize,
        options: RowOptions,
    ) -> egui::Response {
        let inline = opens_in_sidebar(r);
        let (_, rect) = ui.allocate_space(Vec2::new(options.width.max(0.), ROW_HEIGHT));
        let response = ui.interact(
            rect,
            ui.make_persistent_id(("open-artifact", &p.id, &r.target, index)),
            Sense::click(),
        );
        #[cfg(test)]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("dense-resource-row", &p.id, index)), rect)
        });
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                if inline {
                    format!("Open {} in sidebar", r.name)
                } else {
                    format!("Open {} ({})", r.name, r.file_type())
                },
            )
        });
        let mut problem = None;
        if ui.is_rect_visible(rect) {
            let painter = ui.painter_at(rect);
            if response.hovered() || response.has_focus() {
                painter.rect_filled(rect, 0., Color32::from_rgb(29, 43, 52));
            }
            // Every row reserves the same visual column, including while a
            // thumbnail is loading or unavailable. Smaller icons are centered
            // inside that column so text never shifts with the resource type.
            let visual = Rect::from_center_size(
                Pos2::new(rect.left() + 30., rect.center().y),
                Vec2::new(46., 32.),
            );
            let icon_rect = Rect::from_center_size(visual.center(), Vec2::splat(21.));
            let mut caption = resource_caption(r);
            if inline {
                let entry = self.entry(ui, p, r);
                match &entry.content {
                    Content::Image(texture) => {
                        let size = texture.size_vec2();
                        let scale = (visual.width() / size.x).min(visual.height() / size.y);
                        painter.image(
                            texture.id(),
                            Rect::from_center_size(visual.center(), size * scale),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.)),
                            Color32::WHITE,
                        );
                    }
                    Content::Text { rendered, source } => {
                        icon(&painter, icon_rect, r);
                        caption = if options.render_markdown {
                            rendered.clone()
                        } else {
                            source.clone()
                        };
                    }
                    Content::Loading => {
                        icon(&painter, icon_rect, r);
                        caption = "Loading preview…".into();
                    }
                    Content::Unavailable(message) => {
                        icon(&painter, icon_rect, r);
                        caption = "Preview unavailable".into();
                        problem = Some(message.clone());
                    }
                }
            } else {
                icon(&painter, icon_rect, r);
            }
            if p.readiness != "ready" {
                caption = format!("{} · {caption}", p.readiness);
            }
            let left = visual.right() + 8.;
            let name_width = (rect.right() - left - 86.).max(0.);
            paint_line(
                ui,
                &painter,
                Pos2::new(left, rect.top() + 6.),
                name_width,
                &r.name,
                13.,
                INK,
            );
            paint_line(
                ui,
                &painter,
                Pos2::new(left, rect.top() + 23.),
                (rect.right() - left - 5.).max(0.),
                &caption,
                11.,
                MUTED,
            );
            let timestamp = age(p.created_at)
                .replace(" ago", "")
                .replace("Just now", "now");
            painter.text(
                Pos2::new(rect.right() - 5., rect.top() + 8.),
                Align2::RIGHT_TOP,
                timestamp,
                FontId::monospace(10.),
                MUTED,
            );
            let status_x = rect.right() - 63.;
            if !p.read {
                painter.circle_filled(Pos2::new(status_x, rect.top() + 12.), 2., BLUE);
            }
            if p.pinned {
                painter.text(
                    Pos2::new(status_x - 13., rect.top() + 6.),
                    Align2::RIGHT_TOP,
                    "◆",
                    FontId::monospace(10.),
                    MUTED,
                );
            }
            if p.discovery.is_some() {
                painter.circle_stroke(
                    Pos2::new(status_x - 6., rect.top() + 12.),
                    2.,
                    Stroke::new(1.0_f32, MUTED),
                );
            }
            if options.selected {
                painter.rect_stroke(
                    rect,
                    0.,
                    Stroke::new(1.0_f32, MUTED),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let mut tooltip = format!(
            "{}\n{} ({})\n{}\n{}",
            p.title,
            p.context.label,
            p.context.agent,
            r.target,
            if inline {
                "Click to open in Feedr's sidebar"
            } else {
                "Click to open in its default application"
            }
        );
        if p.resources.len() > 1 {
            tooltip.push_str(&format!(
                "\nAttachment {} of {}",
                index + 1,
                p.resources.len()
            ));
        }
        if let Some(origin) = &p.discovery {
            tooltip.push_str(&format!(
                "\n{}: {}",
                if origin.accepted {
                    "Kept mention"
                } else {
                    "Discovered mention"
                },
                origin.excerpt
            ));
        }
        if let Some(problem) = problem {
            tooltip.push_str(&format!("\n{problem}"));
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tooltip)
    }
}

use std::path::Path;
fn resource_caption(resource: &Resource) -> String {
    if resource.kind == ResourceKind::Url {
        return resource.target.clone();
    }
    let location = Path::new(&resource.target)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(&resource.target);
    format!("{} · {location}", resource.file_type())
}
fn paint_line(
    ui: &egui::Ui,
    painter: &egui::Painter,
    position: Pos2,
    width: f32,
    text: &str,
    size: f32,
    color: Color32,
) {
    let mut job = egui::text::LayoutJob::simple(
        text.replace(['\n', '\r'], " "),
        FontId::monospace(size),
        color,
        width,
    );
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    painter.galley(position, galley, color);
}
fn excerpt(text: &str, rendered: bool) -> String {
    let plain = if rendered {
        let mut plain = String::new();
        let mut metadata = false;
        for event in Parser::new_ext(
            text,
            Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
                | Options::ENABLE_PLUSES_DELIMITED_METADATA_BLOCKS,
        ) {
            match event {
                Event::Start(Tag::MetadataBlock(_)) => metadata = true,
                Event::End(TagEnd::MetadataBlock(_)) => metadata = false,
                Event::Text(value) | Event::Code(value) if !metadata => {
                    plain.push_str(&value);
                    plain.push(' ');
                }
                _ => {}
            }
            if plain.len() > 1024 {
                break;
            }
        }
        plain
    } else {
        text.chars().take(1024).collect()
    };
    plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(320)
        .collect()
}

fn icon(p: &egui::Painter, r: Rect, resource: &Resource) {
    let at = |x: f32, y: f32| r.min + Vec2::new(x * r.width(), y * r.height());
    let stroke = Stroke::new(1.5_f32, BLUE);
    match resource.kind {
        ResourceKind::Folder => {
            let points = vec![
                at(0.05, 0.17),
                at(0.37, 0.17),
                at(0.49, 0.31),
                at(0.95, 0.31),
                at(0.95, 0.87),
                at(0.05, 0.87),
            ];
            p.add(egui::Shape::convex_polygon(
                points,
                BLUE.gamma_multiply(0.16),
                stroke,
            ));
            p.line_segment([at(0.05, 0.4), at(0.95, 0.4)], stroke);
        }
        ResourceKind::Url => {
            p.circle_stroke(r.center(), r.width() * 0.41, stroke);
            p.add(egui::Shape::ellipse_stroke(
                r.center(),
                Vec2::new(r.width() * 0.20, r.height() * 0.41),
                stroke,
            ));
            p.line_segment([at(0.09, 0.5), at(0.91, 0.5)], stroke);
        }
        ResourceKind::App => {
            p.rect_stroke(r.shrink(2.), 5., stroke, egui::StrokeKind::Inside);
            for x in [0.3, 0.7] {
                for y in [0.3, 0.7] {
                    p.rect_filled(Rect::from_center_size(at(x, y), r.size() * 0.20), 2., BLUE);
                }
            }
        }
        ResourceKind::File => {
            let points = vec![
                at(0.17, 0.04),
                at(0.62, 0.04),
                at(0.84, 0.27),
                at(0.84, 0.96),
                at(0.17, 0.96),
            ];
            p.add(egui::Shape::convex_polygon(
                points,
                BLUE.gamma_multiply(0.1),
                stroke,
            ));
            p.line_segment([at(0.62, 0.04), at(0.62, 0.27)], stroke);
            p.line_segment([at(0.62, 0.27), at(0.84, 0.27)], stroke);
            p.line_segment([at(0.30, 0.49), at(0.70, 0.49)], stroke);
            p.line_segment([at(0.30, 0.66), at(0.62, 0.66)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::excerpt;

    #[test]
    fn markdown_excerpts_hide_metadata_and_markup_but_can_show_source() {
        let text = "---\nname: internal\nsecret: metadata\n---\n# Preview guide\n\nOpen **images** and [reports](./report.pdf).";
        let rendered = excerpt(text, true);
        assert!(rendered.starts_with("Preview guide"));
        assert!(rendered.contains("images"));
        assert!(rendered.contains("reports"));
        assert!(!rendered.contains("metadata"));
        assert!(!rendered.contains("report.pdf"));
        let source = excerpt(text, false);
        assert!(source.contains("# Preview guide"));
        assert!(source.contains("**images**"));
        assert!(source.contains("metadata"));
        assert_eq!(excerpt(&"界".repeat(500), true).chars().count(), 320);
    }
}
