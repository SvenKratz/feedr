use super::{BLUE, INK, MUTED};
use crate::update::{self, Github, Plan, Release};
use eframe::egui::{self, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CHECK_INTERVAL: u64 = 6 * 60 * 60;
enum Phase {
    Idle,
    Checking,
    Downloading(u64, u64),
    Ready(Box<Plan>, Option<Instant>),
    Launching,
    Error(String),
}
enum Event {
    Checked(Result<Option<Release>, String>),
    Progress(u64, u64),
    Prepared(Result<Plan, String>),
    Launched(Result<(), String>),
}
pub(super) struct Updates {
    root: PathBuf,
    pub automatic: bool,
    pub next_check: u64,
    open: bool,
    checked: bool,
    release: Option<Release>,
    phase: Phase,
    events: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    notes: CommonMarkCache,
    failed_version: Option<String>,
    health_sent: bool,
}
impl Updates {
    pub fn new(root: PathBuf, automatic: bool, next_check: u64) -> Self {
        let status = std::fs::read(root.join("updates/status.json"))
            .ok()
            .filter(|s| s.len() < 64 * 1024)
            .and_then(|s| serde_json::from_slice::<serde_json::Value>(&s).ok());
        let error = status
            .as_ref()
            .and_then(|v| v["error"].as_str())
            .map(str::to_owned);
        let failed_version = if error.is_some() {
            status
                .as_ref()
                .and_then(|v| v["version"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        Self {
            root,
            automatic,
            next_check,
            open: false,
            checked: false,
            release: None,
            phase: error.map(Phase::Error).unwrap_or(Phase::Idle),
            events: None,
            cancel: Arc::new(AtomicBool::new(false)),
            notes: CommonMarkCache::default(),
            failed_version,
            health_sent: false,
        }
    }
    fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking | Phase::Downloading(..) | Phase::Launching | Phase::Ready(..)
        )
    }
    fn newer(&self) -> bool {
        self.release
            .as_ref()
            .is_some_and(|r| r.is_newer_than(env!("CARGO_PKG_VERSION")).unwrap_or(false))
    }
    fn check(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Checking;
        self.next_check = now() + CHECK_INTERVAL;
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = Github::new()
                .and_then(|g| g.latest())
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::Checked(result));
            ctx.request_repaint();
        });
    }
    fn download(&mut self, ctx: &egui::Context) {
        let Some(release) = self.release.clone() else {
            return;
        };
        self.phase = Phase::Downloading(0, 0);
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        let root = self.root.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        thread::spawn(move || {
            let result = (|| {
                let exe = std::env::current_exe()?;
                update::prepare(
                    &Github::new()?,
                    &release,
                    &exe,
                    &root,
                    &cancel,
                    |bytes, total| {
                        let _ = tx.send(Event::Progress(bytes, total));
                        ctx.request_repaint();
                    },
                )
            })()
            .map_err(|e: anyhow::Error| format!("{e:#}"));
            let _ = tx.send(Event::Prepared(result));
            ctx.request_repaint();
        });
    }
    fn install(&mut self, ctx: &egui::Context) {
        let Phase::Ready(plan, ..) = &self.phase else {
            return;
        };
        let plan = plan.clone();
        self.phase = Phase::Launching;
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = update::launch(&plan).map_err(|e| format!("{e:#}"));
            if result.is_err() {
                plan.discard();
            }
            let _ = tx.send(Event::Launched(result));
            ctx.request_repaint();
        });
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        if !self.health_sent && ctx.cumulative_frame_nr() >= 2 {
            self.health_sent = true;
            if let Ok(id) = std::env::var("FEEDR_UPDATE_HEALTH_ID")
                && uuid::Uuid::parse_str(&id).is_ok()
            {
                let dir = self.root.join("updates");
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(format!("health-{id}")), env!("CARGO_PKG_VERSION"));
                self.phase = Phase::Idle;
                self.failed_version = None;
            }
        }
        let events: Vec<_> = self
            .events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Checked(Ok(release)) => {
                    self.checked = true;
                    self.release = release;
                    self.phase = Phase::Idle;
                    if self.automatic
                        && self.newer()
                        && self.failed_version.as_ref().is_none_or(|v| {
                            self.release.as_ref().is_none_or(|r| {
                                r.version().map(|r| r.to_string() != *v).unwrap_or(false)
                            })
                        })
                        && self.installation_issue().is_none()
                    {
                        self.download(ctx);
                    }
                }
                Event::Progress(bytes, total) => self.phase = Phase::Downloading(bytes, total),
                Event::Prepared(Ok(plan)) => {
                    self.phase = Phase::Ready(
                        Box::new(plan),
                        self.automatic
                            .then(|| Instant::now() + Duration::from_secs(5)),
                    );
                    self.open = true;
                }
                Event::Launched(Ok(())) => return true,
                Event::Checked(Err(e)) | Event::Prepared(Err(e)) | Event::Launched(Err(e)) => {
                    self.phase = Phase::Error(e);
                    self.next_check = now() + 60 * 60;
                }
            }
        }
        let install = matches!(&self.phase,Phase::Ready(_,Some(deadline)) if self.automatic && Instant::now()>=*deadline);
        if install {
            self.install(ctx);
        }
        if matches!(self.phase, Phase::Ready(_, Some(_))) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.automatic && now() >= self.next_check && !self.busy() {
            self.check(ctx);
        }
        false
    }
    fn installation_issue(&self) -> Option<String> {
        let result = (|| -> anyhow::Result<()> {
            update::destination(&std::env::current_exe()?)?;
            if let Some(r) = &self.release {
                r.asset(std::env::consts::OS, std::env::consts::ARCH)?;
            }
            Ok(())
        })();
        result.err().map(|e| format!("{e:#}"))
    }
    pub fn button(&mut self, ui: &mut egui::Ui) {
        let label = match self.phase {
            Phase::Checking => "Checking…",
            Phase::Downloading(..) => "Downloading update…",
            Phase::Ready(..) | Phase::Launching => "Update ready",
            Phase::Error(..) => "Update issue",
            _ if self.newer() => "Update available",
            _ => concat!("v", env!("CARGO_PKG_VERSION")),
        };
        if ui
            .add(
                egui::Button::new(RichText::new(label).size(11.).color(if self.newer() {
                    BLUE
                } else {
                    MUTED
                }))
                .frame(false),
            )
            .on_hover_text("Feedr updates and release notes")
            .clicked()
        {
            self.open = !self.open;
            if self.open && !self.checked && !self.busy() {
                self.check(ui.ctx());
            }
        }
    }
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = true;
        egui::Window::new("Feedr updates")
            .id(egui::Id::new("feedr-updates"))
            .open(&mut open)
            .default_width(500.)
            .max_width(680.)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading(format!("Feedr {}", env!("CARGO_PKG_VERSION")));
                ui.add_space(6.);
                let changed = ui
                    .checkbox(
                        &mut self.automatic,
                        "Download and install updates automatically",
                    )
                    .changed();
                if changed {
                    if let Phase::Ready(_, deadline) = &mut self.phase {
                        *deadline = self
                            .automatic
                            .then(|| Instant::now() + Duration::from_secs(5));
                    } else if self.automatic {
                        self.next_check = 0;
                    }
                }
                ui.label(
                    RichText::new("Checks every 6 hours. Feedr restarts after a verified update.")
                        .size(11.)
                        .color(MUTED),
                );
                ui.add_space(10.);
                match &mut self.phase {
                    Phase::Checking => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Checking GitHub releases…");
                        });
                    }
                    Phase::Downloading(bytes, total) => {
                        ui.label("Downloading and verifying the update…");
                        ui.add(
                            egui::ProgressBar::new(*bytes as f32 / (*total).max(1) as f32)
                                .show_percentage(),
                        );
                    }
                    Phase::Ready(_, deadline) => {
                        ui.horizontal(|ui| {
                            if let Some(at) = deadline.filter(|_| self.automatic) {
                                ui.label(format!(
                                    "Update verified. Restarting in {}s…",
                                    at.saturating_duration_since(Instant::now()).as_secs() + 1
                                ));
                                if ui.small_button("Later").clicked() {
                                    *deadline = None;
                                }
                            } else {
                                ui.label("Update verified and ready to install.");
                            }
                        });
                        if ui.button("Install and restart now").clicked() {
                            self.install(ctx);
                        }
                    }
                    Phase::Launching => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Restarting Feedr…");
                        });
                    }
                    Phase::Error(message) => {
                        ui.colored_label(egui::Color32::from_rgb(235, 166, 142), message.as_str());
                    }
                    Phase::Idle => {
                        ui.label(if self.newer() {
                            "A new release is available."
                        } else if self.release.is_some() {
                            "You're up to date."
                        } else if self.checked {
                            "No releases have been published yet."
                        } else {
                            "Ready to check for updates."
                        });
                    }
                }
                if self.newer() && !self.busy() {
                    if let Some(issue) = self.installation_issue() {
                        ui.label(RichText::new(issue).size(12.).color(MUTED));
                    } else if ui.button("Download update").clicked() {
                        self.download(ctx);
                    }
                }
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new("Check now"))
                        .clicked()
                    {
                        self.check(ctx);
                    }
                    if ui.button("View releases on GitHub").clicked() {
                        let _ = open::that_detached(update::RELEASES_URL);
                    }
                });
                if let Some(release) = &self.release {
                    ui.add_space(10.);
                    ui.separator();
                    ui.label(
                        RichText::new(
                            release
                                .name
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .unwrap_or(&release.tag_name),
                        )
                        .strong()
                        .color(INK),
                    );
                    if let Some(date) = &release.published_at {
                        ui.label(
                            RichText::new(date.split('T').next().unwrap_or(date))
                                .size(11.)
                                .color(MUTED),
                        );
                    }
                    egui::ScrollArea::vertical()
                        .id_salt("release-notes")
                        .max_height(360.)
                        .show(ui, |ui| {
                            CommonMarkViewer::new().show(
                                ui,
                                &mut self.notes,
                                release
                                    .body
                                    .as_deref()
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("No release notes were published for this version."),
                            );
                        });
                }
            });
        self.open = open;
    }
}
impl Drop for Updates {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Phase::Ready(plan, ..) = &self.phase {
            plan.discard();
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
