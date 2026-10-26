//! Selected PDF/video viewers own their work and cancel it when closed.
use crate::media::{
    self,
    video::{Control, Player},
};
use eframe::egui::{self, RichText};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

struct Page {
    number: usize,
    image: Result<egui::ColorImage, String>,
}
pub(super) struct Pdf {
    jobs: mpsc::SyncSender<usize>,
    pages: mpsc::Receiver<Page>,
    info: mpsc::Receiver<Result<usize, String>>,
    count: Option<usize>,
    selected: usize,
    textures: HashMap<usize, egui::TextureHandle>,
    requested: HashSet<usize>,
    errors: HashMap<usize, String>,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
    zoom: f32,
}
impl Pdf {
    pub fn new(path: PathBuf, ctx: egui::Context) -> Self {
        let (jobs, work) = mpsc::sync_channel::<usize>(8);
        let (tx, pages) = mpsc::channel();
        let (metadata, info) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = cancel.clone();
        thread::spawn(move || {
            let info = media::pdf::pages(&path, &stopping).map_err(|e| format!("{e:#}"));
            let valid = info.is_ok();
            let _ = metadata.send(info);
            ctx.request_repaint();
            if !valid {
                return;
            }
            while let Ok(number) = work.recv() {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let image = media::pdf::render(&path, number, 2200, &stopping)
                    .map_err(|e| format!("{e:#}"));
                if tx.send(Page { number, image }).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            jobs,
            pages,
            info,
            count: None,
            selected: 1,
            textures: HashMap::new(),
            requested: HashSet::new(),
            errors: HashMap::new(),
            error: None,
            cancel,
            zoom: 1.,
        }
    }
    fn request(&mut self, page: usize) {
        if !self.requested.contains(&page) && self.jobs.try_send(page).is_ok() {
            self.requested.insert(page);
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if let Ok(info) = self.info.try_recv() {
            match info {
                Ok(count) => self.count = Some(count),
                Err(e) => self.error = Some(e),
            }
        }
        while let Ok(page) = self.pages.try_recv() {
            match page.image {
                Ok(image) => {
                    self.textures.insert(
                        page.number,
                        ui.ctx().load_texture(
                            format!("pdf-page-{}", page.number),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                    );
                }
                Err(error) => {
                    self.errors.insert(page.number, error);
                }
            }
        }
        // Nine cached full-size pages at most; thumbnails reuse those textures.
        while self.textures.len() > 9 {
            if let Some(page) = self
                .textures
                .keys()
                .max_by_key(|p| p.abs_diff(self.selected))
                .copied()
            {
                self.textures.remove(&page);
                self.requested.remove(&page);
            }
        }
        if let Some(error) = &self.error {
            ui.colored_label(super::MUTED, error);
            return;
        }
        let Some(count) = self.count else {
            ui.spinner();
            ui.label("Reading PDF…");
            return;
        };
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.selected > 1, egui::Button::new("Previous page"))
                .clicked()
            {
                self.selected -= 1;
            }
            ui.label("Page");
            ui.add(egui::DragValue::new(&mut self.selected).range(1..=count));
            ui.label(format!("of {count}"));
            if ui
                .add_enabled(self.selected < count, egui::Button::new("Next page"))
                .clicked()
            {
                self.selected += 1;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Zoom");
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=3.).logarithmic(true));
            if ui.small_button("Fit").clicked() {
                self.zoom = 1.;
            }
        });
        self.request(self.selected);
        let start = self.selected.saturating_sub(2).max(1);
        let end = (start + 4).min(count);
        egui::ScrollArea::horizontal()
            .id_salt("pdf-thumbnails")
            .max_height(110.)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for page in start..=end {
                        self.request(page);
                        ui.vertical(|ui| {
                            if let Some(texture) = self.textures.get(&page) {
                                let size = texture.size_vec2();
                                let response = ui.add(
                                    egui::Button::image(
                                        egui::Image::new(texture)
                                            .fit_to_exact_size(size * (65. / size.y)),
                                    )
                                    .selected(self.selected == page),
                                );
                                if response
                                    .on_hover_text(format!("Go to page {page}"))
                                    .clicked()
                                {
                                    self.selected = page;
                                }
                            } else if ui
                                .add_sized([48., 65.], egui::Button::new(page.to_string()))
                                .clicked()
                            {
                                self.selected = page;
                            }
                            ui.label(RichText::new(page.to_string()).small().color(super::MUTED));
                        });
                    }
                });
            });
        if let Some(error) = self.errors.get(&self.selected) {
            ui.colored_label(super::MUTED, error);
            return;
        }
        if let Some(texture) = self.textures.get(&self.selected) {
            image_canvas(ui, texture, self.zoom, ("pdf", self.selected), false, false);
        } else {
            ui.spinner();
            ui.label("Rendering page…");
        }
    }
}
impl Drop for Pdf {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(super) struct Video {
    player: Player,
    texture: Option<egui::TextureHandle>,
    seek: f64,
    seeking: bool,
    clicks: SurfaceClicks,
}
impl Video {
    pub fn new(path: &Path, ctx: egui::Context) -> Self {
        Self {
            player: Player::new(path, ctx),
            texture: None,
            seek: 0.,
            seeking: false,
            clicks: SurfaceClicks::default(),
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let (status, frame) = self.player.receive();
        if let Some(image) = frame {
            if let Some(texture) = &mut self.texture {
                texture.set(image, egui::TextureOptions::LINEAR);
            } else {
                self.texture = Some(ui.ctx().load_texture(
                    "video-frame",
                    image,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        if let Some(error) = status.error {
            ui.label(error);
            return false;
        }
        let mut clicked = false;
        let mut double_clicked = false;
        if let Some(texture) = &self.texture {
            let size = texture.size_vec2();
            let scale = (ui.available_width() / size.x)
                .min((ui.available_height() - 125.).max(120.) / size.y);
            let area = egui::vec2(
                ui.available_width(),
                (ui.available_height() - 125.).max(120.),
            );
            let (rect, response) = ui.allocate_exact_size(area, egui::Sense::click());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 3., super::PANEL);
            painter.image(
                texture.id(),
                egui::Rect::from_center_size(rect.center(), size * scale),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                egui::Color32::WHITE,
            );
            let response =
                response.on_hover_text("Click to play / pause; double-click to expand or return");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Video preview")
            });
            #[cfg(test)]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("video-preview-rect"), response.rect)
            });
            clicked = response.clicked();
            double_clicked = response.double_clicked();
        } else {
            ui.spinner();
            ui.label("Loading video…");
        }
        let action = self.clicks.update(
            clicked,
            double_clicked,
            ui.input(|input| input.time),
            ui.ctx()
                .options(|options| options.input_options.max_double_click_delay),
        );
        let toggle = action == SurfaceAction::TogglePlayback;
        let expanded = action == SurfaceAction::ToggleExpanded;
        ui.horizontal(|ui| {
            if ui
                .button(if status.paused { "Play" } else { "Pause" })
                .clicked()
                || toggle
            {
                self.clicks.pending = None;
                if status.paused && status.duration > 0. && status.position >= status.duration - 0.1
                {
                    self.player.control(Control::Seek(0.));
                }
                self.player.control(Control::Pause(!status.paused));
            }
            ui.label(format!(
                "{} / {}",
                timestamp(status.position),
                timestamp(status.duration)
            ));
            ui.label(
                RichText::new(if status.audio {
                    "Audio"
                } else {
                    "No audio track"
                })
                .small()
                .color(super::MUTED),
            );
        });
        let position = &mut self.seek;
        if !self.seeking {
            *position = status.position;
        }
        let seek = ui.add(
            egui::Slider::new(position, 0.0..=status.duration.max(0.001))
                .show_value(false)
                .text("Seek"),
        );
        if seek.dragged() {
            self.seeking = true;
        }
        if seek.drag_stopped() || (seek.changed() && !seek.dragged()) {
            self.player.control(Control::Seek(*position));
            self.seeking = false;
        }
        let mut volume = status.volume;
        if ui
            .add(egui::Slider::new(&mut volume, 0.0..=100.).text("Volume"))
            .changed()
        {
            self.player.control(Control::Volume(volume));
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
        expanded
    }
}

#[derive(Default)]
struct SurfaceClicks {
    pending: Option<f64>,
}
#[derive(Debug, PartialEq, Eq)]
enum SurfaceAction {
    None,
    TogglePlayback,
    ToggleExpanded,
}
impl SurfaceClicks {
    fn update(
        &mut self,
        clicked: bool,
        double_clicked: bool,
        now: f64,
        delay: f64,
    ) -> SurfaceAction {
        if double_clicked {
            self.pending = None;
            return SurfaceAction::ToggleExpanded;
        }
        if clicked {
            self.pending = Some(now);
        }
        // Wait out the double-click interval before toggling playback, so the
        // first click of an expand gesture cannot start or pause the video.
        if self.pending.is_some_and(|at| now - at >= delay) {
            self.pending = None;
            SurfaceAction::TogglePlayback
        } else {
            SurfaceAction::None
        }
    }
}
fn timestamp(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

pub(super) enum Viewer {
    Pdf(Pdf),
    Video(Video),
}
impl Viewer {
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        match self {
            Self::Pdf(pdf) => {
                pdf.show(ui);
                false
            }
            Self::Video(video) => video.show(ui),
        }
    }
}

pub(super) fn markdown_base(target: &str) -> String {
    Path::new(target)
        .parent()
        .and_then(|path| url::Url::from_directory_path(path).ok())
        .map(|url| url.to_string())
        .unwrap_or_else(|| "file://".into())
}

pub(super) fn image_canvas(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    zoom: f32,
    id: impl std::hash::Hash,
    expanded: bool,
    expandable: bool,
) -> egui::Response {
    let size = egui::vec2(
        ui.available_width(),
        if expanded {
            ui.available_height().max(1.)
        } else {
            ui.available_height().clamp(180., 620.)
        },
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            if expanded {
                "Expanded image preview"
            } else {
                "Image preview"
            },
        )
    });
    #[cfg(test)]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new(("image-preview-rect", expanded)), rect));
    let id = ui.make_persistent_id(("image-pan", id, expanded));
    let mut pan = ui
        .ctx()
        .data_mut(|data| data.get_temp::<egui::Vec2>(id).unwrap_or_default());
    let natural = texture.size_vec2();
    let scale = (rect.width() / natural.x).min(rect.height() / natural.y);
    let scale = if expanded { scale } else { scale.min(1.) };
    let displayed = natural * scale * zoom;
    if response.dragged() {
        pan += ui.ctx().input(|input| input.pointer.delta());
    }
    if response.double_clicked() {
        pan = egui::Vec2::ZERO;
    }
    let limit = ((displayed - rect.size()) * 0.5).max(egui::Vec2::ZERO);
    pan = pan.clamp(-limit, limit);
    ui.ctx().data_mut(|data| data.insert_temp(id, pan));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3., super::PANEL);
    painter.image(
        texture.id(),
        egui::Rect::from_center_size(rect.center() + pan, displayed),
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
        egui::Color32::WHITE,
    );
    response.on_hover_text(if !expandable {
        "Drag to pan; double-click to center"
    } else if expanded {
        "Drag to pan; double-click to return"
    } else {
        "Drag to pan; double-click to expand"
    })
}

#[cfg(test)]
mod tests {
    use super::{SurfaceAction, SurfaceClicks};

    #[test]
    fn video_double_click_expands_without_toggling_playback() {
        let mut clicks = SurfaceClicks::default();
        assert_eq!(clicks.update(true, false, 1., 0.3), SurfaceAction::None);
        assert_eq!(clicks.update(false, false, 1.1, 0.3), SurfaceAction::None);
        assert_eq!(
            clicks.update(true, true, 1.2, 0.3),
            SurfaceAction::ToggleExpanded
        );
        assert_eq!(clicks.update(false, false, 1.6, 0.3), SurfaceAction::None);
        assert_eq!(clicks.update(true, false, 2., 0.3), SurfaceAction::None);
        assert_eq!(
            clicks.update(false, false, 2.4, 0.3),
            SurfaceAction::TogglePlayback
        );
        assert_eq!(clicks.update(false, false, 2.5, 0.3), SurfaceAction::None);
    }
}
