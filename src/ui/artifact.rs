//! Resource surfaces are the primary click targets in the feed.
use super::{Palette, Preview, age, load_preview, opens_in_sidebar, resource_text};
use crate::model::{Post, Resource, ResourceKind};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt,
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
pub(super) type Stamp = Option<(Option<SystemTime>, u64, u64, u64)>;

// Size catches writes within one filesystem timestamp; identity also catches
// atomic replacements that preserve the previous size and modification time.
pub(super) fn stamp(post: &Post, resource: &Resource) -> Stamp {
    if !post.context.is_local() {
        return None;
    }
    fs::metadata(&resource.target)
        .ok()
        .map(|m| (m.modified().ok(), m.len(), m.dev(), m.ino()))
}
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
            let stamp = stamp(post, r);
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
        let colors = Palette::get(ui.ctx());
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
                painter.rect_filled(rect, 0., colors.hover);
            }
            // Every row reserves the same visual column, including while a
            // thumbnail is loading or unavailable. Smaller icons are centered
            // inside that column so text never shifts with the resource type.
            let visual = Rect::from_center_size(
                Pos2::new(rect.left() + 30., rect.center().y),
                Vec2::new(46., 32.),
            );
            let icon_rect = Rect::from_center_size(visual.center(), Vec2::splat(21.));
            let left = visual.right() + 8.;
            let caption_width = (rect.right() - left - 5.).max(0.);
            let readiness = if p.readiness == "ready" {
                String::new()
            } else {
                format!("{} · ", p.readiness)
            };
            let mut caption = resource_caption(
                ui,
                r,
                (caption_width - resource_text::width(ui, &readiness, 11.)).max(0.),
            );
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
                        if r.file_type() == "Video" {
                            video_badge(&painter, visual, colors);
                        }
                    }
                    Content::Text { rendered, source } => {
                        icon(&painter, icon_rect, r, colors.accent);
                        caption = if options.render_markdown {
                            rendered.clone()
                        } else {
                            source.clone()
                        };
                    }
                    Content::Loading => {
                        icon(&painter, icon_rect, r, colors.accent);
                        caption = "Loading preview…".into();
                    }
                    Content::Unavailable(message) => {
                        icon(&painter, icon_rect, r, colors.accent);
                        caption = "Preview unavailable".into();
                        problem = Some(message.clone());
                    }
                }
            } else {
                icon(&painter, icon_rect, r, colors.accent);
            }
            caption = format!("{readiness}{caption}");
            let name_width = (rect.right() - left - 86.).max(0.);
            let name = if r.kind == ResourceKind::Url {
                r.name.clone()
            } else {
                resource_text::tail(ui, &r.name, 13., name_width)
            };
            paint_line(
                ui,
                &painter,
                Pos2::new(left, rect.top() + 6.),
                name_width,
                &name,
                13.,
                colors.ink,
            );
            paint_line(
                ui,
                &painter,
                Pos2::new(left, rect.top() + 23.),
                caption_width,
                &caption,
                11.,
                colors.muted,
            );
            let timestamp = age(p.created_at)
                .replace(" ago", "")
                .replace("Just now", "now");
            painter.text(
                Pos2::new(rect.right() - 5., rect.top() + 8.),
                Align2::RIGHT_TOP,
                timestamp,
                FontId::monospace(10.),
                colors.muted,
            );
            let status_x = rect.right() - 63.;
            if !p.read {
                painter.circle_filled(Pos2::new(status_x, rect.top() + 12.), 2., colors.accent);
            }
            if p.pinned {
                painter.text(
                    Pos2::new(status_x - 13., rect.top() + 6.),
                    Align2::RIGHT_TOP,
                    "◆",
                    FontId::monospace(10.),
                    colors.muted,
                );
            }
            if p.discovery.is_some() {
                painter.circle_stroke(
                    Pos2::new(status_x - 6., rect.top() + 12.),
                    2.,
                    Stroke::new(1.0_f32, colors.muted),
                );
            }
            if options.selected || response.has_focus() {
                painter.rect_stroke(
                    rect,
                    0.,
                    Stroke::new(
                        1.0_f32,
                        if response.has_focus() {
                            colors.accent
                        } else {
                            colors.muted
                        },
                    ),
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
fn resource_caption(ui: &egui::Ui, resource: &Resource, width: f32) -> String {
    if resource.kind == ResourceKind::Url {
        return resource.target.clone();
    }
    let location = Path::new(&resource.target)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or(&resource.target);
    let prefix = format!("{} · ", resource.file_type());
    let location = resource_text::tail(
        ui,
        location,
        11.,
        (width - resource_text::width(ui, &prefix, 11.)).max(0.),
    );
    format!("{prefix}{location}")
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

fn icon(p: &egui::Painter, r: Rect, resource: &Resource, accent: Color32) {
    let at = |x: f32, y: f32| r.min + Vec2::new(x * r.width(), y * r.height());
    let stroke = Stroke::new(1.5_f32, accent);
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
                accent.gamma_multiply(0.16),
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
                    p.rect_filled(
                        Rect::from_center_size(at(x, y), r.size() * 0.20),
                        2.,
                        accent,
                    );
                }
            }
        }
        ResourceKind::File if resource.file_type() == "Audio" => {
            for (x, y) in [(0.27, 0.79), (0.73, 0.66)] {
                p.add(egui::Shape::ellipse_filled(
                    at(x, y),
                    Vec2::new(r.width() * 0.16, r.height() * 0.11),
                    accent,
                ));
            }
            p.line_segment([at(0.41, 0.77), at(0.41, 0.19)], stroke);
            p.line_segment([at(0.87, 0.64), at(0.87, 0.06)], stroke);
            p.line_segment([at(0.41, 0.19), at(0.87, 0.06)], stroke);
            p.line_segment([at(0.41, 0.32), at(0.87, 0.19)], stroke);
        }
        ResourceKind::File if resource.file_type() == "Video" => {
            p.rect_stroke(
                Rect::from_min_max(at(0.04, 0.16), at(0.96, 0.84)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::convex_polygon(
                vec![at(0.40, 0.34), at(0.40, 0.66), at(0.67, 0.50)],
                accent,
                Stroke::NONE,
            ));
            for x in [0.16, 0.84] {
                for y in [0.31, 0.50, 0.69] {
                    p.rect_filled(
                        Rect::from_center_size(at(x, y), r.size() * 0.07),
                        0.,
                        accent,
                    );
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
                accent.gamma_multiply(0.1),
                stroke,
            ));
            p.line_segment([at(0.62, 0.04), at(0.62, 0.27)], stroke);
            p.line_segment([at(0.62, 0.27), at(0.84, 0.27)], stroke);
            p.line_segment([at(0.30, 0.49), at(0.70, 0.49)], stroke);
            p.line_segment([at(0.30, 0.66), at(0.62, 0.66)], stroke);
        }
    }
}

fn video_badge(p: &egui::Painter, thumbnail: Rect, colors: Palette) {
    let center = thumbnail.right_bottom() - Vec2::splat(6.);
    p.circle_filled(center, 6., colors.panel);
    p.circle_stroke(center, 6., Stroke::new(1.0_f32, colors.accent));
    p.add(egui::Shape::convex_polygon(
        vec![
            center + Vec2::new(-1.5, -2.5),
            center + Vec2::new(-1.5, 2.5),
            center + Vec2::new(2.5, 0.),
        ],
        colors.accent,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_post(path: &std::path::Path) -> (Post, Resource) {
        let post = crate::ui::tests::post();
        let mut resource = post.resources[1].clone();
        resource.target = path.to_string_lossy().into();
        resource.original = resource.target.clone();
        (post, resource)
    }

    fn write_image(path: &std::path::Path, width: u32) {
        image::RgbaImage::from_pixel(width, 4, image::Rgba([80, 160, 180, 255]))
            .save(path)
            .unwrap();
    }

    fn visit(cache: &mut Artifacts, ctx: &egui::Context, post: &Post, resources: &[Resource]) {
        cache.receive(ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for resource in resources {
                    cache.entry(ui, post, resource);
                }
            });
        });
    }

    fn until(cache: &mut Artifacts, ctx: &egui::Context, post: &Post, r: &Resource, image: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            visit(cache, ctx, post, std::slice::from_ref(r));
            let entry = &cache.entries[&format!("{}:{}", post.context.host, r.target)];
            if (image && matches!(entry.content, Content::Image(_)))
                || (!image && matches!(entry.content, Content::Unavailable(_)))
            {
                return;
            }
            assert!(Instant::now() < deadline, "thumbnail did not converge");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn recheck(cache: &mut Artifacts) {
        for entry in cache.entries.values_mut() {
            entry.checked = Instant::now() - Duration::from_secs(2);
        }
    }

    #[test]
    fn thumbnail_recovers_from_missing_partial_removed_and_recreated_images() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        let (post, resource) = image_post(&path);
        let ctx = egui::Context::default();
        let mut cache = Artifacts::new();
        until(&mut cache, &ctx, &post, &resource, false);
        fs::write(&path, b"incomplete PNG").unwrap();
        recheck(&mut cache);
        until(&mut cache, &ctx, &post, &resource, false);
        write_image(&path, 8);
        recheck(&mut cache);
        until(&mut cache, &ctx, &post, &resource, true);
        fs::remove_file(&path).unwrap();
        recheck(&mut cache);
        until(&mut cache, &ctx, &post, &resource, false);
        write_image(&path, 12);
        recheck(&mut cache);
        until(&mut cache, &ctx, &post, &resource, true);
    }

    #[test]
    fn atomic_image_replacement_invalidates_thumbnail_even_with_preserved_mtime() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        write_image(&path, 8);
        let (post, resource) = image_post(&path);
        let original = fs::metadata(&path).unwrap();
        let ctx = egui::Context::default();
        let mut cache = Artifacts::new();
        until(&mut cache, &ctx, &post, &resource, true);
        let key = format!("{}:{}", post.context.host, resource.target);
        let previous = cache.entries[&key].stamp;
        let replacement = root.path().join("replacement.png");
        write_image(&replacement, 12);
        fs::File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(original.modified().unwrap()))
            .unwrap();
        fs::rename(replacement, &path).unwrap();
        assert_ne!(stamp(&post, &resource), previous);
        recheck(&mut cache);
        until(&mut cache, &ctx, &post, &resource, true);
        assert!(
            matches!(&cache.entries[&key].content, Content::Image(texture) if texture.size() == [12, 4])
        );
    }

    #[test]
    fn stale_thumbnail_results_cannot_replace_a_newer_image() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("image.png");
        write_image(&path, 8);
        let (post, resource) = image_post(&path);
        let current = stamp(&post, &resource);
        let key = format!("{}:{}", post.context.host, resource.target);
        let (jobs, _rx) = mpsc::sync_channel(1);
        let (tx, results) = mpsc::channel();
        let mut cache = Artifacts {
            entries: HashMap::from([(
                key.clone(),
                Entry {
                    stamp: current,
                    checked: Instant::now(),
                    queued: true,
                    content: Content::Loading,
                    seen: 0,
                },
            )]),
            jobs,
            results,
        };
        let ctx = egui::Context::default();
        tx.send(Finished {
            key: key.clone(),
            stamp: None,
            preview: Preview::Error("obsolete decode failure".into()),
        })
        .unwrap();
        cache.receive(&ctx);
        assert!(matches!(cache.entries[&key].content, Content::Loading));
        tx.send(Finished {
            key: key.clone(),
            stamp: current,
            preview: Preview::Image(egui::ColorImage::filled([8, 4], Color32::LIGHT_BLUE)),
        })
        .unwrap();
        cache.receive(&ctx);
        assert!(matches!(cache.entries[&key].content, Content::Image(_)));
    }

    #[test]
    fn a_burst_larger_than_the_thumbnail_queue_eventually_loads_every_image() {
        let root = tempfile::tempdir().unwrap();
        let (post, _) = image_post(&root.path().join("unused.png"));
        let resources: Vec<_> = (0..96)
            .map(|i| {
                let path = root.path().join(format!("image-{i}.png"));
                write_image(&path, 8);
                image_post(&path).1
            })
            .collect();
        let mut cache = Artifacts::new();
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            visit(&mut cache, &ctx, &post, &resources);
            if cache.entries.len() == 96
                && cache
                    .entries
                    .values()
                    .all(|e| matches!(e.content, Content::Image(_)))
            {
                break;
            }
            assert!(Instant::now() < deadline, "thumbnail queue lost work");
            thread::sleep(Duration::from_millis(2));
        }
    }

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
