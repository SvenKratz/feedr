use crate::{broker::Client, model::*, platform};
use anyhow::Result;
use eframe::egui::{self, Color32, RichText, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

mod artifact;
mod feed_columns;
use feed_columns::{Feed, Layout as ColumnLayout, Target as FeedTarget};
mod media;
mod session_color;
mod session_list;

const INK: Color32 = Color32::from_rgb(217, 229, 238);
const MUTED: Color32 = Color32::from_rgb(144, 164, 181);
const BLUE: Color32 = Color32::from_rgb(121, 201, 203);
const LINE: Color32 = Color32::from_rgb(43, 59, 73);
const PAPER: Color32 = Color32::from_rgb(12, 17, 22);
const PANEL: Color32 = Color32::from_rgb(16, 24, 31);

enum Command {
    Call(&'static str, serde_json::Value),
    Query(Vec<(FeedTarget, FeedQuery)>),
    Update(UpdateInput),
    Refresh,
}
enum Event {
    Snapshot(FeedTarget, FeedQuery, Box<Snapshot>),
    Error(String),
}
enum Preview {
    Text(String),
    Image(egui::ColorImage),
    Unsupported(String),
    Error(String),
}

pub fn run(client: Client) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1260., 820.])
            .with_min_inner_size([800., 560.])
            .with_app_id("dev.feedr.desktop"),
        ..Default::default()
    };
    eframe::run_native(
        "Feedr",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, client)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
struct App {
    artifacts: artifact::Artifacts,
    hotkey: Option<global_hotkey::GlobalHotKeyManager>,
    snapshot: Snapshot,
    columns: ColumnLayout,
    feeds: HashMap<FeedTarget, Feed>,
    reveal_column: Option<FeedTarget>,
    dragging_column: Option<(FeedTarget, egui::Pos2)>,
    query: FeedQuery,
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
    selected: Option<String>,
    reveal_session: Option<String>,
    reveal_artifact: bool,
    session_colors: HashMap<String, String>,
    session_view: session_list::View,
    resource: usize,
    error: Option<String>,
    search_focus: bool,
    preview_key: String,
    preview_rx: Option<mpsc::Receiver<Preview>>,
    preview_active: Arc<AtomicUsize>,
    preview: Option<Preview>,
    texture: Option<egui::TextureHandle>,
    markdown: CommonMarkCache,
    render_markdown: bool,
    media: Option<media::Viewer>,
    media_key: String,
    zoom: f32,
    full_resolution: bool,
    expanded_preview: bool,
    expanded_zoom: f32,
    show_help: bool,
    connections: bool,
    show_discovery: bool,
}
impl App {
    fn new(cc: &eframe::CreationContext<'_>, client: Client) -> Self {
        let ctx = &cc.egui_ctx;
        egui_extras::install_image_loaders(ctx);
        let render_markdown = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "feedr.render_markdown"))
            .unwrap_or(true);
        let session_colors = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "feedr.session_colors"))
            .unwrap_or_default();
        let session_view = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "feedr.session_view"))
            .unwrap_or_default();
        let show_unavailable = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "feedr.show_unavailable"))
            .unwrap_or(false);
        let query = FeedQuery {
            available_only: !show_unavailable,
            ..FeedQuery::default()
        };
        let mut columns: ColumnLayout = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, "feedr.columns"))
            .unwrap_or_default();
        columns.normalize();
        let feeds: HashMap<_, _> = columns
            .order
            .iter()
            .cloned()
            .map(|t| (t, Feed::default()))
            .collect();
        let subscriptions: Vec<_> = columns
            .order
            .iter()
            .map(|t| (t.clone(), feeds[t].query(&query, t)))
            .collect();
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.override_text_color = Some(INK);
        style.visuals.panel_fill = PAPER;
        style.visuals.window_fill = PANEL;
        style.visuals.hyperlink_color = BLUE;
        style.visuals.code_bg_color = Color32::from_rgb(10, 20, 28);
        style.visuals.selection.bg_fill = Color32::from_rgb(34, 59, 73);
        style.visuals.selection.stroke = egui::Stroke::new(1., BLUE);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 43, 54);
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(28, 43, 54);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 61, 72);
        style.spacing.item_spacing = Vec2::new(9., 8.);
        style.spacing.button_padding = Vec2::new(11., 6.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::monospace(14.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::monospace(13.));
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::monospace(23.));
        ctx.set_style(style);
        let hotkey = register_summon(ctx)
            .map_err(|e| eprintln!("Quick summon unavailable: {e:#}"))
            .ok();
        let (tx, rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let context = ctx.clone();
        thread::spawn(move || {
            let mut queries = subscriptions;
            let mut refresh = true;
            let mut sequence = -1;
            let mut last_availability_refresh = Instant::now();
            while !stopping.load(Ordering::Relaxed) {
                while let Ok(command) = rx.try_recv() {
                    match command {
                        Command::Call(method, params) => {
                            if let Err(error) = client.call::<serde_json::Value>(method, params) {
                                let _ = event_tx.send(Event::Error(format!("{error:#}")));
                            }
                            refresh = true;
                        }
                        Command::Query(q) => {
                            queries = q;
                            refresh = true;
                        }
                        Command::Refresh => refresh = true,
                        Command::Update(u) => {
                            if let Err(e) = client
                                .call::<Post>("update", serde_json::to_value(u).unwrap_or_default())
                            {
                                let _ = event_tx.send(Event::Error(e.to_string()));
                            }
                            refresh = true;
                        }
                    }
                }
                if queries.iter().any(|(_, query)| query.available_only)
                    && last_availability_refresh.elapsed() >= Duration::from_secs(2)
                {
                    refresh = true;
                }
                if refresh {
                    last_availability_refresh = Instant::now();
                    // Still refresh session metadata when the user closes every column.
                    let requests = if queries.is_empty() {
                        vec![(
                            FeedTarget::All,
                            FeedQuery {
                                limit: 1,
                                ..FeedQuery::default()
                            },
                        )]
                    } else {
                        queries.clone()
                    };
                    for (index, (target, query)) in requests.into_iter().enumerate() {
                        match client.snapshot(&query) {
                            Ok(s) => {
                                // Waiting from the first result also catches changes between queries.
                                if index == 0 {
                                    sequence = s.sequence;
                                }
                                let _ = event_tx.send(Event::Snapshot(target, query, Box::new(s)));
                            }
                            Err(e) => {
                                let _ =
                                    event_tx.send(Event::Error(format!("Feed unavailable: {e:#}")));
                            }
                        }
                    }
                    context.request_repaint();
                    refresh = false;
                }
                match client
                    .call::<serde_json::Value>("wait", json!({"after":sequence,"timeout_ms":200}))
                {
                    Ok(v) => {
                        if v["sequence"].as_i64().unwrap_or(-1) != sequence {
                            refresh = true;
                        }
                    }
                    Err(_) => {
                        thread::sleep(Duration::from_millis(500));
                        let _ = client.ensure_running();
                        refresh = true;
                    }
                }
            }
        });
        Self {
            artifacts: artifact::Artifacts::new(),
            hotkey,
            snapshot: Snapshot::default(),
            columns,
            feeds,
            reveal_column: None,
            dragging_column: None,
            query,
            commands: tx,
            events: event_rx,
            stop,
            selected: None,
            reveal_session: None,
            reveal_artifact: false,
            session_colors,
            session_view,
            resource: 0,
            error: None,
            search_focus: false,
            preview_key: String::new(),
            preview_rx: None,
            preview_active: Arc::new(AtomicUsize::new(0)),
            preview: None,
            texture: None,
            markdown: CommonMarkCache::default(),
            render_markdown,
            media: None,
            media_key: String::new(),
            zoom: 1.,
            full_resolution: false,
            expanded_preview: false,
            expanded_zoom: 1.,
            show_help: false,
            connections: false,
            show_discovery: false,
        }
    }
    fn subscribe_columns(&mut self) {
        self.feeds
            .retain(|target, _| self.columns.order.contains(target));
        let queries = self
            .columns
            .order
            .iter()
            .map(|target| {
                let feed = self.feeds.entry(target.clone()).or_default();
                (target.clone(), feed.query(&self.query, target))
            })
            .collect();
        let _ = self.commands.send(Command::Query(queries));
    }
    fn send_query(&mut self) {
        self.expanded_preview = false;
        self.selected = None;
        self.media = None;
        self.media_key.clear();
        self.reveal_session = None;
        self.reveal_artifact = false;
        self.feeds.clear();
        self.snapshot.posts.clear();
        self.subscribe_columns();
    }
    fn open_feed(&mut self, target: FeedTarget) {
        self.columns.open(target.clone());
        self.reveal_column = Some(target);
        self.subscribe_columns();
    }
    fn close_feed(&mut self, target: &FeedTarget) {
        self.columns.close(target);
        self.subscribe_columns();
        self.merge_posts();
    }
    fn merge_posts(&mut self) {
        let selected_target = self
            .selected
            .as_ref()
            .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
            .and_then(|post| post.resources.get(self.resource))
            .map(|resource| resource.target.clone());
        let mut posts: HashMap<String, Post> = HashMap::new();
        for feed in self.feeds.values() {
            for post in &feed.snapshot.posts {
                if posts
                    .get(&post.id)
                    .is_none_or(|old| old.revision < post.revision)
                {
                    posts.insert(post.id.clone(), post.clone());
                }
            }
        }
        self.snapshot.posts = posts.into_values().collect();
        self.snapshot
            .posts
            .sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        if let Some(target) = selected_target {
            if let Some(index) = self
                .selected
                .as_ref()
                .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
                .and_then(|p| p.resources.iter().position(|r| r.target == target))
            {
                self.resource = index;
            } else {
                self.selected = None;
            }
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.snapshot.posts.iter().any(|p| &p.id == id))
        {
            self.selected = None;
        }
    }
    fn edit(&self, p: &Post, change: impl FnOnce(&mut UpdateInput)) {
        let mut u = UpdateInput {
            id: p.id.clone(),
            expected_revision: p.revision,
            title: None,
            body: None,
            resources: None,
            readiness: None,
            pinned: None,
            archived: None,
            read: None,
        };
        change(&mut u);
        let _ = self.commands.send(Command::Update(u));
    }
    fn active_session(&self) -> Option<&str> {
        self.selected.as_ref().and_then(|id| {
            self.snapshot
                .posts
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.session_id.as_str())
        })
    }
    fn select(&mut self, id: String) {
        self.expanded_preview = false;
        self.zoom = 1.;
        self.full_resolution = false;
        let session = self
            .snapshot
            .posts
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.session_id.clone());
        if self.active_session() != session.as_deref() {
            self.reveal_session = session;
        }
        self.selected = Some(id.clone());
        self.resource = 0;
        self.preview_key.clear();
        if let Some(p) = self.snapshot.posts.iter().find(|p| p.id == id && !p.read) {
            self.edit(p, |u| u.read = Some(true));
        }
    }
    fn select_resource(&mut self, p: &Post, index: usize) {
        if self.selected.as_deref() != Some(&p.id) || self.resource != index {
            self.select(p.id.clone());
            self.resource = index;
        }
    }
    fn navigate_artifact(&mut self, down: bool) {
        let Some(posts) = self
            .columns
            .active
            .as_ref()
            .and_then(|target| self.feeds.get(target))
            .map(|feed| &feed.snapshot.posts)
        else {
            return;
        };
        let rows: Vec<_> = posts
            .iter()
            .flat_map(|post| {
                post.resources
                    .iter()
                    .enumerate()
                    .filter(|(_, resource)| {
                        self.query
                            .kind
                            .as_ref()
                            .is_none_or(|kind| resource.file_type() == kind)
                    })
                    .map(move |(index, _)| (post, index))
            })
            .collect();
        let selected = rows.iter().position(|(post, index)| {
            self.selected.as_deref() == Some(&post.id) && self.resource == *index
        });
        let next = if down {
            selected.map_or(0, |index| (index + 1).min(rows.len().saturating_sub(1)))
        } else {
            selected.unwrap_or(1).saturating_sub(1)
        };
        if let Some((post, index)) = rows.get(next) {
            let (post, index) = ((*post).clone(), *index);
            self.select_resource(&post, index);
            self.reveal_artifact = true;
        }
    }
    fn activate_artifact(&mut self, p: &Post, index: usize) {
        let Some(resource) = p.resources.get(index) else {
            return;
        };
        if opens_in_sidebar(resource) {
            self.select_resource(p, index);
        } else {
            self.open_externally(p, resource);
        }
    }
    fn receive(&mut self) {
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Error(e) => self.error = Some(e),
                Event::Snapshot(target, query, snapshot) => {
                    self.snapshot.sessions = snapshot.sessions.clone();
                    self.snapshot.connections = snapshot.connections.clone();
                    self.snapshot.discovery = snapshot.discovery.clone();
                    self.snapshot.discovery_error = snapshot.discovery_error.clone();
                    let Some(feed) = self.feeds.get_mut(&target) else {
                        continue;
                    };
                    if query != feed.query(&self.query, &target) {
                        continue;
                    }
                    let reading = self
                        .selected
                        .as_ref()
                        .is_some_and(|id| feed.snapshot.posts.iter().any(|p| &p.id == id));
                    feed.receive(*snapshot, reading);
                    changed = true;
                }
            }
        }
        if changed {
            self.merge_posts();
        }
    }
    fn prepare_preview(&mut self, ctx: &egui::Context, p: &Post, r: &Resource) {
        let modified = fs::metadata(&r.target).and_then(|m| m.modified()).ok();
        let key = format!("{}:{}:{modified:?}", p.context.host, r.target);
        if key == self.preview_key {
            return;
        }
        if self.preview_active.load(Ordering::Relaxed) >= 2 {
            self.preview = None;
            self.texture = None;
            self.preview_rx = None;
            self.preview_key.clear();
            return;
        }
        self.preview_key = key;
        self.preview = None;
        self.texture = None;
        self.markdown = CommonMarkCache::default();
        let (tx, rx) = mpsc::channel();
        self.preview_rx = Some(rx);
        let p = p.clone();
        let r = r.clone();
        let ctx = ctx.clone();
        let active = self.preview_active.clone();
        let image_size = if self.full_resolution { 16000 } else { 2400 };
        active.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let preview = load_preview(&p, &r, image_size);
            let _ = tx.send(preview);
            active.fetch_sub(1, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(14, 7))
                    .stroke(egui::Stroke::new(1., LINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = egui::vec2(8., 3.);
                    ui.label(RichText::new("feedr").size(22.).strong().color(BLUE));
                    let search_width = (ui.available_width() - 420.).clamp(120., 360.);
                    let search = ui.add(
                        egui::TextEdit::singleline(&mut self.query.search)
                            .hint_text("Search all feed columns…")
                            .desired_width(search_width)
                            .margin(egui::Margin::symmetric(8, 4)),
                    );
                    if self.search_focus {
                        search.request_focus();
                        self.search_focus = false;
                    }
                    if search.changed() {
                        self.send_query();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("?")
                            .on_hover_text("Publishing and keyboard shortcuts")
                            .clicked()
                        {
                            self.show_help = !self.show_help;
                        }
                        if ui
                            .button(if self.snapshot.discovery.enabled {
                                "Discovery on"
                            } else {
                                "Discovery off"
                            })
                            .clicked()
                        {
                            self.show_discovery = !self.show_discovery;
                        }
                        let connected = self
                            .snapshot
                            .connections
                            .iter()
                            .filter(|s| s.connected)
                            .count();
                        if ui
                            .button(
                                RichText::new(if connected > 0 {
                                    format!("Herdr connected ({connected})")
                                } else {
                                    "Waiting for herdr".into()
                                })
                                .color(if connected > 0 {
                                    Color32::from_rgb(126, 190, 140)
                                } else {
                                    MUTED
                                }),
                            )
                            .clicked()
                        {
                            self.connections = !self.connections;
                        }
                    });
                });
            });
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        let active_session = self.active_session().map(str::to_owned);
        egui::SidePanel::left("sessions")
            .exact_width(224.)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(16))
            .show(ctx, |ui| {
                ui.add_space(8.);
                ui.label(RichText::new("Your feed").strong().size(16.));
                ui.add_space(10.);
                if ui
                    .selectable_label(
                        self.columns.active.as_ref() == Some(&FeedTarget::All)
                            && !self.query.pinned_only
                            && !self.query.archived,
                        "All resources",
                    )
                    .clicked()
                {
                    let filters_changed = self.query.pinned_only || self.query.archived;
                    self.query.pinned_only = false;
                    self.query.archived = false;
                    if filters_changed {
                        self.send_query();
                    }
                    self.open_feed(FeedTarget::All);
                }
                if ui
                    .selectable_label(self.query.pinned_only, "Pinned")
                    .clicked()
                {
                    self.query.pinned_only = !self.query.pinned_only;
                    self.query.archived = false;
                    self.send_query();
                }
                if ui
                    .selectable_label(self.query.archived, "Archived")
                    .clicked()
                {
                    self.query.archived = !self.query.archived;
                    self.send_query();
                }
                ui.add_space(22.);
                ui.label(RichText::new("Agent sessions").color(MUTED).size(12.));
                ui.add_space(4.);
                let previous_view = self.session_view;
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut self.session_view,
                        session_list::View::Priority,
                        RichText::new("Priority").size(12.),
                    )
                    .on_hover_text("Blocked, done, working, idle; newest state changes first");
                    ui.selectable_value(
                        &mut self.session_view,
                        session_list::View::Grouped,
                        RichText::new("Grouped").size(12.),
                    )
                    .on_hover_text("Herdr workspace and pane order");
                });
                if previous_view != self.session_view {
                    self.reveal_session = active_session.clone();
                }
                let posted: HashSet<_> = self
                    .snapshot
                    .posts
                    .iter()
                    .map(|p| p.session_id.as_str())
                    .collect();
                let mut sessions: Vec<_> = self
                    .snapshot
                    .sessions
                    .iter()
                    .filter(|s| {
                        (s.online && s.context.agent != "Shell") || posted.contains(s.id.as_str())
                    })
                    .cloned()
                    .collect();
                session_list::sort(&mut sessions, self.session_view);
                egui::ScrollArea::vertical()
                    .id_salt(("session_scroll", self.session_view))
                    .show(ui, |ui| {
                        let mut previous_group = None;
                        for session in &sessions {
                            let group = session_list::group(session);
                            if self.session_view == session_list::View::Grouped
                                && previous_group != Some(group)
                            {
                                ui.add_space(8.);
                                ui.label(
                                    RichText::new(ellipsize(
                                        session_list::group_label(session),
                                        24,
                                    ))
                                    .strong()
                                    .size(12.)
                                    .color(MUTED),
                                )
                                .on_hover_text(format!(
                                    "{}\n{}\n{}",
                                    session_list::group_label(session),
                                    session.context.host,
                                    session
                                        .context
                                        .herdr_socket
                                        .as_deref()
                                        .unwrap_or("Local posts"),
                                ));
                                previous_group = Some(group);
                            }
                            ui.push_id(&session.id, |ui| {
                                let active = active_session.as_deref() == Some(&session.id);
                                let filtered = self.columns.active.as_ref()
                                    == Some(&FeedTarget::Session(session.id.clone()));
                                let highlighted = active || filtered;
                                let color =
                                    session_color::resolve(&session.id, &self.session_colors);
                                let text = format!(
                                    "{} {}\n{}",
                                    session_list::status_label(session),
                                    session.context.agent,
                                    ellipsize(&session.context.label, 26)
                                );
                                let response = ui.add_sized(
                                    [ui.available_width(), 38.],
                                    egui::Button::new(RichText::new(text).size(11.).color(INK))
                                        .selected(highlighted)
                                        .fill(if highlighted {
                                            color.highlighted()
                                        } else {
                                            color.sidebar()
                                        })
                                        .stroke(egui::Stroke::new(
                                            if active { 2. } else { 1. },
                                            if highlighted {
                                                color.accent
                                            } else {
                                                color.border()
                                            },
                                        ))
                                        .corner_radius(4),
                                );
                                ui.painter().rect_filled(
                                    egui::Rect::from_min_max(
                                        response.rect.min + egui::vec2(0., 6.),
                                        egui::pos2(
                                            response.rect.left() + 3.,
                                            response.rect.bottom() - 6.,
                                        ),
                                    ),
                                    2.,
                                    color.accent,
                                );
                                if self.reveal_session.as_deref() == Some(&session.id) {
                                    response.scroll_to_me(Some(egui::Align::Center));
                                    self.reveal_session = None;
                                }
                                response.context_menu(|ui| {
                                    if ui.button("Add Feed Column").clicked() {
                                        self.open_feed(FeedTarget::Session(session.id.clone()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    ui.label(RichText::new("Session color").color(MUTED).size(12.));
                                    for choice in session_color::PALETTE {
                                        if ui
                                            .selectable_label(
                                                color.id == choice.id,
                                                RichText::new(format!("●  {}", choice.name))
                                                    .color(choice.accent),
                                            )
                                            .clicked()
                                        {
                                            self.session_colors
                                                .insert(session.id.clone(), choice.id.into());
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
                                    if ui
                                        .add_enabled(
                                            self.session_colors.contains_key(&session.id),
                                            egui::Button::new("Use automatic color"),
                                        )
                                        .clicked()
                                    {
                                        self.session_colors.remove(&session.id);
                                        ui.close();
                                    }
                                });
                                if response
                                    .on_hover_text(format!(
                                        "{}\n{}\n{}\n{} · Right-click for feed and color options",
                                        session.context.label,
                                        session.context.cwd,
                                        session.status,
                                        color.name
                                    ))
                                    .clicked()
                                {
                                    self.open_feed(FeedTarget::Session(session.id.clone()));
                                }
                            });
                        }
                    });
            });
    }
    fn add_feed_button(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("+", |ui| {
            ui.set_max_width(420.);
            if ui.button("All sessions").clicked() {
                self.open_feed(FeedTarget::All);
                ui.close();
            }
            ui.separator();
            let mut sessions = self.snapshot.sessions.clone();
            session_list::sort(&mut sessions, self.session_view);
            egui::ScrollArea::vertical()
                .max_height(400.)
                .show(ui, |ui| {
                    for session in sessions {
                        if session.context.agent == "Shell" {
                            continue;
                        }
                        let target = FeedTarget::Session(session.id.clone());
                        let open = self.columns.order.contains(&target);
                        if ui
                            .add(
                                egui::Button::new(target.title(&self.snapshot.sessions))
                                    .selected(open)
                                    .truncate(),
                            )
                            .on_hover_text(&session.context.label)
                            .clicked()
                        {
                            self.open_feed(target);
                            ui.close();
                        }
                    }
                });
        })
        .response
        .on_hover_text("Add feed column");
    }
    fn feed(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PAPER).inner_margin(8))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for kind in [
                        "All", "Markdown", "Image", "PDF", "Video", "Web", "Folder", "File",
                    ] {
                        let selected = self.query.kind.as_deref().unwrap_or("All") == kind;
                        if ui.selectable_label(selected, kind).clicked() {
                            self.query.kind = if kind == "All" {
                                None
                            } else {
                                Some(kind.into())
                            };
                            self.send_query();
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.add_feed_button(ui);
                        ui.menu_button("View", |ui| {
                            let mut show = !self.query.available_only;
                            if ui.checkbox(&mut show, "Show unavailable files").changed() {
                                self.query.available_only = !show;
                                self.send_query();
                            }
                        });
                    });
                });
                if let Some(error) = &self.error.clone() {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(Color32::from_rgb(235, 142, 145), error);
                        if ui.small_button("Dismiss").clicked() {
                            self.error = None;
                        }
                    });
                }
                ui.add_space(6.);
                if self.columns.order.is_empty() {
                    ui.add_space(60.);
                    ui.heading("Choose a feed to keep within reach.");
                    ui.label("Use + above or right-click a session to add a feed column.");
                    if ui.button("Add All sessions feed").clicked() {
                        self.open_feed(FeedTarget::All);
                    }
                    return;
                }
                let order = self.columns.order.clone();
                // Fit two or three readable columns; overflow horizontally at narrow widths.
                let gap = 8.;
                let width = ((ui.available_width() - gap * (order.len() - 1) as f32)
                    / order.len() as f32)
                    .max(340.);
                let height = ui.available_height() - 14.;
                egui::ScrollArea::horizontal()
                    .id_salt("feed_columns_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        ui.horizontal_top(|ui| {
                            let mut moved = None;
                            let mut closed = None;
                            let mut drop_targets = Vec::new();
                            for target in order {
                                ui.push_id(&target, |ui| {
                                    let (rect, close) =
                                        self.feed_column(ui, &target, width, height);
                                    drop_targets.push((target.clone(), rect));
                                    if close {
                                        closed = Some(target.clone());
                                    }
                                });
                            }
                            // Track the press origin as well as release: fast drags can
                            // finish before egui reports a separate drag-start frame.
                            if ui.input(|input| input.pointer.primary_released()) {
                                if let Some((source, origin)) = self.dragging_column.take()
                                    && let Some(pointer) = ui.ctx().pointer_interact_pos()
                                    && pointer.distance(origin) > 6.
                                    && let Some((destination, rect)) = drop_targets
                                        .into_iter()
                                        .find(|(_, rect)| rect.contains(pointer))
                                {
                                    moved =
                                        Some((source, destination, pointer.x > rect.center().x));
                                }
                            } else if !ui.input(|input| input.pointer.primary_down()) {
                                self.dragging_column = None;
                            }
                            if let Some((source, destination, after)) = moved {
                                self.columns.move_to(&source, &destination, after);
                            }
                            if let Some(target) = closed {
                                self.close_feed(&target);
                            }
                        });
                    });
            });
    }
    fn feed_column(
        &mut self,
        ui: &mut egui::Ui,
        target: &FeedTarget,
        width: f32,
        height: f32,
    ) -> (egui::Rect, bool) {
        let mut closed = false;
        let column = ui.allocate_ui_with_layout(
            egui::vec2(width, height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let color = target
                    .session_id()
                    .map(|id| session_color::resolve(id, &self.session_colors));
                let accent = color.map_or(MUTED, |c| c.accent);
                let background = color.map_or(PANEL, |c| c.background);
                let active = self.columns.active.as_ref() == Some(target);
                let (total, unread) = self
                    .feeds
                    .get(target)
                    .map(|feed| (feed.snapshot.total, feed.snapshot.unread))
                    .unwrap_or_default();
                egui::Frame::new()
                    .fill(background)
                    .corner_radius(3)
                    .inner_margin(egui::Margin::symmetric(4, 0))
                    .show(ui, |ui| {
                        ui.set_width(width - 8.);
                        ui.set_min_height(height);
                        ui.spacing_mut().item_spacing = egui::vec2(4., 2.);
                        let header = egui::Frame::new()
                            .fill(background)
                            .stroke(egui::Stroke::new(
                                1.,
                                if active { accent } else { background },
                            ))
                            .corner_radius(4)
                            .inner_margin(egui::Margin::symmetric(3, 1))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.x = 4.;
                                ui.spacing_mut().button_padding.y = 2.;
                                ui.horizontal(|ui| {
                                    let label = target.title(&self.snapshot.sessions);
                                    let compact = target.compact_title(
                                        &self.snapshot.sessions,
                                        ((ui.available_width() - 110.) / 8.).max(12.) as usize,
                                    );
                                    let count = total.to_string();
                                    let count_width = count.len() as f32 * 7. + 6.;
                                    let handle = line_action(
                                        ui,
                                        RichText::new(&compact).color(accent).size(13.),
                                        ui.available_width() - 34. - count_width,
                                        22.,
                                        24.,
                                        egui::Sense::click_and_drag(),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::Grab)
                                    .on_hover_text(format!(
                                        "{label}\nDrag to rearrange feed columns"
                                    ));
                                    for x in [8., 12.] {
                                        for y in [-4., 0., 4.] {
                                            ui.painter().circle_filled(
                                                egui::pos2(
                                                    handle.rect.left() + x,
                                                    handle.rect.center().y + y,
                                                ),
                                                1.,
                                                accent,
                                            );
                                        }
                                    }
                                    handle.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            true,
                                            &label,
                                        )
                                    });
                                    // Inspect the press event's position, even if a fast native
                                    // gesture delivers press, movement and release together.
                                    let press = ui.input(|input| {
                                        input.events.iter().find_map(|event| match event {
                                            egui::Event::PointerButton {
                                                pos,
                                                button: egui::PointerButton::Primary,
                                                pressed: true,
                                                ..
                                            } => Some(*pos),
                                            _ => None,
                                        })
                                    });
                                    if let Some(origin) = press
                                        && handle.rect.contains(origin)
                                        && ui.clip_rect().contains(origin)
                                        && ui.ctx().layer_id_at(origin) == Some(ui.layer_id())
                                    {
                                        self.dragging_column = Some((target.clone(), origin));
                                        self.columns.active = Some(target.clone());
                                    }
                                    if handle.clicked() {
                                        self.columns.active = Some(target.clone());
                                    }
                                    ui.label(RichText::new(count).size(11.).color(MUTED))
                                        .on_hover_text(format!("{total} posts / {unread} unread"));
                                    if ui
                                        .add_sized([25., 22.], egui::Button::new("×").frame(false))
                                        .on_hover_text(format!("Close {label} feed column"))
                                        .clicked()
                                    {
                                        closed = true;
                                    }
                                });
                            });
                        if self.reveal_column.as_ref() == Some(target) {
                            header.response.scroll_to_me(Some(egui::Align::Center));
                            self.reveal_column = None;
                        }
                        #[cfg(test)]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(
                                egui::Id::new(("feed-column-rect", &target)),
                                header.response.rect,
                            )
                        });
                        if let Some((source, origin)) = &self.dragging_column
                            && let Some(pointer) = ui.ctx().pointer_interact_pos()
                            && pointer.distance(*origin) > 6.
                        {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            if source != target && header.response.rect.contains(pointer) {
                                let x = if pointer.x > header.response.rect.center().x {
                                    header.response.rect.right()
                                } else {
                                    header.response.rect.left()
                                };
                                ui.painter().vline(
                                    x,
                                    header.response.rect.y_range(),
                                    egui::Stroke::new(3., accent),
                                );
                            }
                        }
                        let Some(feed) = self.feeds.get(target) else {
                            return header.response.rect;
                        };
                        let count = feed
                            .pending
                            .as_ref()
                            .map(|pending| {
                                pending
                                    .posts
                                    .iter()
                                    .filter(|p| {
                                        !feed.snapshot.posts.iter().any(|old| old.id == p.id)
                                    })
                                    .count()
                            })
                            .unwrap_or(0);
                        if count > 0
                            && ui
                                .small_button(format!("{count} new resources — show latest"))
                                .clicked()
                        {
                            let feed = self.feeds.get_mut(target).unwrap();
                            feed.snapshot = feed.pending.take().unwrap();
                            self.merge_posts();
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("posts")
                            .auto_shrink([false, false])
                            .max_height(ui.available_height())
                            .show(ui, |ui| {
                                ui.set_width(width - 8.);
                                let Some(feed) = self.feeds.get(target) else {
                                    return;
                                };
                                let posts = feed.snapshot.posts.clone();
                                let remaining = feed.snapshot.total.saturating_sub(posts.len());
                                if posts.is_empty() {
                                    ui.add_space(24.);
                                    ui.label(if self.query.search.is_empty() {
                                        "No resources in this feed yet."
                                    } else {
                                        "No matching resources."
                                    });
                                    if ui.button("Show publishing setup").clicked() {
                                        self.show_help = true;
                                    }
                                }
                                for post in posts {
                                    self.card(ui, &post, target);
                                }
                                if remaining > 0
                                    && ui
                                        .button(format!("Load more ({remaining} remaining)"))
                                        .clicked()
                                {
                                    let feed = self.feeds.get_mut(target).unwrap();
                                    feed.limit = (feed.limit.max(200) + 200).min(2000);
                                    self.subscribe_columns();
                                }
                            });
                        header.response.rect
                    })
                    .inner
            },
        );
        (column.inner, closed)
    }
    fn card(&mut self, ui: &mut egui::Ui, p: &Post, target: &FeedTarget) {
        for (index, resource) in p.resources.iter().enumerate() {
            if self
                .query
                .kind
                .as_ref()
                .is_some_and(|kind| resource.file_type() != kind)
            {
                continue;
            }
            ui.push_id((&p.id, index), |ui| {
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(3, 0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.;
                        ui.spacing_mut().button_padding = egui::vec2(3., 2.);
                        ui.horizontal(|ui| {
                            let options = artifact::RowOptions {
                                width: ui.available_width() - 32.,
                                selected: self.selected.as_deref() == Some(&p.id)
                                    && self.resource == index,
                                render_markdown: self.render_markdown,
                            };
                            let response = self.artifacts.show(ui, p, resource, index, options);
                            if options.selected
                                && self.reveal_artifact
                                && self.columns.active.as_ref() == Some(target)
                            {
                                response.scroll_to_me(None);
                                self.reveal_artifact = false;
                            }
                            if response.clicked() {
                                self.columns.active = Some(target.clone());
                                self.activate_artifact(p, index);
                                ui.ctx().request_repaint();
                            }
                            if response.secondary_clicked() {
                                self.columns.active = Some(target.clone());
                            }
                            response.context_menu(|ui| self.resource_menu(ui, p, index));
                            let menu = ui
                                .menu_button(RichText::new("···").size(11.).color(MUTED), |ui| {
                                    self.resource_menu(ui, p, index)
                                });
                            menu.response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    true,
                                    format!("Actions for {}", resource.name),
                                )
                            });
                            if menu.response.clicked() {
                                self.columns.active = Some(target.clone());
                            }
                        });
                    });
            });
        }
    }
    fn resource_menu(&mut self, ui: &mut egui::Ui, post: &Post, index: usize) {
        let resource = &post.resources[index];
        ui.label(RichText::new(&post.title).size(12.).strong());
        if post.resources.len() > 1 {
            ui.label(
                RichText::new(format!(
                    "Attachment {} of {}",
                    index + 1,
                    post.resources.len()
                ))
                .size(11.)
                .color(MUTED),
            );
        }
        ui.separator();
        if ui
            .button(if opens_in_sidebar(resource) {
                "Open in sidebar"
            } else {
                "Inspect"
            })
            .clicked()
        {
            self.select_resource(post, index);
            ui.close();
        }
        if ui.button("Open externally").clicked() {
            self.open_externally(post, resource);
            ui.close();
        }
        if resource.kind != ResourceKind::Url && ui.button("Show in folder").clicked() {
            self.action(post, resource, true);
            ui.close();
        }
        if ui.button("Copy location").clicked() {
            ui.ctx().copy_text(resource.target.clone());
            ui.close();
        }
        ui.separator();
        if post
            .discovery
            .as_ref()
            .is_some_and(|origin| !origin.accepted)
            && ui.button("Keep mention").clicked()
        {
            let _ = self.commands.send(Command::Call(
                "discovery.keep",
                json!({"id":post.id,"expected_revision":post.revision}),
            ));
            ui.close();
        }
        if ui
            .button(if post.pinned {
                "Unpin post"
            } else {
                "Pin post"
            })
            .clicked()
        {
            self.edit(post, |u| u.pinned = Some(!post.pinned));
            ui.close();
        }
        let archive = if post.archived {
            "Restore post"
        } else if post
            .discovery
            .as_ref()
            .is_some_and(|origin| !origin.accepted)
        {
            "Dismiss mention"
        } else {
            "Archive post"
        };
        if ui.button(archive).clicked() {
            self.edit(post, |u| u.archived = Some(!post.archived));
            ui.close();
        }
    }
    fn open_externally(&mut self, p: &Post, r: &Resource) {
        self.action(p, r, false);
        if !p.read {
            self.edit(p, |u| u.read = Some(true));
        }
    }
    fn action(&mut self, p: &Post, r: &Resource, reveal: bool) {
        let result = if reveal {
            platform::reveal(p, r)
        } else {
            platform::open_resource(p, r)
        };
        if let Err(e) = result {
            self.error = Some(e.to_string());
        }
    }
    fn configure_discovery(&mut self, config: crate::discovery::Config) {
        self.snapshot.discovery = config.clone();
        let _ = self.commands.send(Command::Call(
            "discovery.configure",
            serde_json::to_value(config).unwrap_or_default(),
        ));
    }
    fn discovery_settings(&mut self, ctx: &egui::Context) {
        if !self.show_discovery {
            return;
        }
        let mut open = self.show_discovery;
        egui::Window::new("Mention discovery").open(&mut open).default_width(440.).show(ctx, |ui| {
            let mut config = self.snapshot.discovery.clone();
            let original = config.clone();
            if ui.checkbox(&mut config.enabled, "Discover mentions in all sessions").changed() {
                config.set_all_sessions(config.enabled);
            }
            ui.label("Choose which resources to surface from agent output.");
            ui.add_space(8.);
            ui.horizontal(|ui| {
                ui.label(RichText::new("File types").strong());
                if ui.small_button("All").clicked() {config.file_types = crate::discovery::FileType::ALL.to_vec();}
                if ui.small_button("None").clicked() {config.file_types.clear();}
            });
            egui::Grid::new("discovery-file-types").num_columns(3).spacing([18.,9.]).show(ui, |ui| {
                for (index,kind) in crate::discovery::FileType::ALL.into_iter().enumerate() {
                    let mut selected = config.file_types.contains(&kind);
                    if ui.checkbox(&mut selected,kind.label()).on_hover_text(kind.description()).changed() {
                        config.file_types.retain(|existing| *existing != kind);
                        if selected {config.file_types.push(kind);}
                    }
                    if index % 3 == 2 {ui.end_row();}
                }
            });
            ui.add_space(8.);
            ui.label(RichText::new("Kept and pinned items stay visible. Other mentions of excluded types are hidden until re-enabled.").color(MUTED).size(12.));
            if config.file_types.is_empty() {ui.label("Select a file type to discover mentions.");}
            ui.separator();
            let mut remove = None;
            ui.collapsing(format!("Watched logs ({})",config.logs.len()), |ui| {
                for (index, log) in config.logs.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| { ui.label(&log.path); if ui.small_button("Remove").clicked() {remove = Some(index);} });
                }
                ui.label("Add log sources with feedr discovery watch-log, or send completed messages with feedr discovery ingest.");
            });
            if let Some(index) = remove { config.logs.remove(index); }
            if let Some(error) = &self.snapshot.discovery_error { ui.separator(); ui.colored_label(MUTED, error); }
            if config != original {self.configure_discovery(config);}
        });
        self.show_discovery = open;
    }
    fn can_expand_preview(&self) -> bool {
        self.selected
            .as_ref()
            .and_then(|id| self.snapshot.posts.iter().find(|post| &post.id == id))
            .and_then(|post| post.resources.get(self.resource))
            .is_some_and(|resource| matches!(resource.file_type(), "Image" | "Video"))
    }
    fn toggle_expanded_preview(&mut self) {
        if self.expanded_preview {
            self.expanded_preview = false;
        } else if self.can_expand_preview() {
            self.expanded_zoom = 1.;
            self.expanded_preview = true;
        }
    }
    fn dismiss_preview(&mut self) {
        if self.expanded_preview {
            self.expanded_preview = false;
        } else {
            self.selected = None;
            self.media = None;
            self.media_key.clear();
        }
        self.dragging_column = None;
        self.reveal_artifact = false;
    }
    fn prepare_selected_viewer(&mut self, ctx: &egui::Context, p: &Post, r: &Resource) -> bool {
        let native_media = matches!(r.file_type(), "PDF" | "Video")
            && p.context.is_local()
            && Path::new(&r.target).is_absolute()
            && Path::new(&r.target).is_file();
        if native_media {
            let modified = fs::metadata(&r.target).and_then(|m| m.modified()).ok();
            let key = format!("{}:{}:{modified:?}", p.context.host, r.target);
            if key != self.media_key {
                self.media = None;
                self.media_key = key;
                self.media = Some(if r.file_type() == "PDF" {
                    media::Viewer::Pdf(media::Pdf::new(r.target.clone().into(), ctx.clone()))
                } else {
                    media::Viewer::Video(media::Video::new(Path::new(&r.target), ctx.clone()))
                });
            }
        } else {
            self.media = None;
            self.media_key.clear();
            self.prepare_preview(ctx, p, r);
        }
        native_media
    }
    fn expanded_viewer(&mut self, ctx: &egui::Context) {
        let Some(post) = self
            .selected
            .as_ref()
            .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
            .cloned()
        else {
            return;
        };
        let Some(resource) = post.resources.get(self.resource).cloned() else {
            return;
        };
        let native_media = self.prepare_selected_viewer(ctx, &post, &resource);
        egui::TopBottomPanel::top("expanded-preview-toolbar")
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .stroke(egui::Stroke::new(1., LINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button("Back to feed")
                        .on_hover_text("Return to the sidebar preview (Esc)")
                        .clicked()
                    {
                        self.expanded_preview = false;
                    }
                    ui.add(
                        egui::Label::new(RichText::new(&resource.name).size(14.).strong())
                            .truncate(),
                    );
                });
                if resource.file_type() == "Image" {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Zoom");
                        ui.add(
                            egui::Slider::new(&mut self.expanded_zoom, 0.25..=8.).logarithmic(true),
                        );
                        if ui.small_button("Fit").clicked() {
                            self.expanded_zoom = 1.;
                        }
                        if ui
                            .checkbox(&mut self.full_resolution, "Load full resolution")
                            .changed()
                        {
                            self.preview_key.clear();
                        }
                    });
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(PAPER).inner_margin(12))
            .show(ctx, |ui| {
                if native_media {
                    if let Some(viewer) = &mut self.media
                        && viewer.show(ui)
                    {
                        self.expanded_preview = false;
                    }
                } else {
                    match &self.preview {
                        Some(Preview::Image(_)) => {
                            if let Some(texture) = &self.texture {
                                let response = media::image_canvas(
                                    ui,
                                    texture,
                                    self.expanded_zoom,
                                    (&post.id, &resource.target),
                                    true,
                                    true,
                                );
                                if response.double_clicked() {
                                    self.expanded_preview = false;
                                }
                            }
                        }
                        Some(Preview::Error(message)) | Some(Preview::Unsupported(message)) => {
                            ui.colored_label(MUTED, message);
                        }
                        _ => {
                            ui.spinner();
                            ui.label("Loading preview…");
                        }
                    }
                }
            });
    }
    fn detail(&mut self, ctx: &egui::Context) {
        let Some(p) = self
            .selected
            .as_ref()
            .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
            .cloned()
        else {
            return;
        };
        self.resource = self.resource.min(p.resources.len().saturating_sub(1));
        let Some(r) = p.resources.get(self.resource).cloned() else {
            return;
        };
        let native_media = self.prepare_selected_viewer(ctx, &p, &r);
        egui::SidePanel::right("detail")
            .default_width(430.)
            .min_width(300.)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(20)
                    .stroke(egui::Stroke::new(1., LINE)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Viewer").color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.selected = None;
                            self.media = None;
                            self.media_key.clear();
                        }
                        if matches!(r.file_type(), "Image" | "Video")
                            && ui
                                .button("Expand")
                                .on_hover_text("Fill the Feedr window")
                                .clicked()
                        {
                            self.toggle_expanded_preview();
                            ctx.request_repaint();
                        }
                    });
                });
                ui.add_space(10.);
                ui.heading(&p.title);
                ui.label(
                    RichText::new(&p.context.label)
                        .color(session_color::resolve(&p.session_id, &self.session_colors).accent),
                );
                if p.resources.len() > 1 {
                    egui::ComboBox::from_id_salt("attachment")
                        .selected_text(&r.name)
                        .show_ui(ui, |ui| {
                            for (i, r) in p.resources.iter().enumerate() {
                                ui.selectable_value(&mut self.resource, i, &r.name);
                            }
                        });
                }
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Open externally").clicked() {
                        self.action(&p, &r, false);
                    }
                    if r.kind != ResourceKind::Url && ui.button("Show in folder").clicked() {
                        self.action(&p, &r, true);
                    }
                    if ui.button("Copy location").clicked() {
                        ctx.copy_text(r.target.clone());
                    }
                });
                ui.add_space(8.);
                ui.label(RichText::new(&r.target).size(11.).color(MUTED));
                ui.separator();
                if native_media {
                    if let Some(viewer) = &mut self.media
                        && viewer.show(ui)
                    {
                        self.toggle_expanded_preview();
                        ctx.request_repaint();
                    }
                    return;
                }
                if r.file_type() == "Markdown" {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.render_markdown, "Render Markdown");
                        if let Some(Preview::Text(text)) = &self.preview
                            && quiet_button(ui, "Copy source").clicked()
                        {
                            ctx.copy_text(text.clone());
                        }
                    });
                    ui.separator();
                }
                egui::ScrollArea::both()
                    .id_salt(("preview_scroll", &p.id, &r.target, self.render_markdown))
                    .show(ui, |ui| match &self.preview {
                        None => {
                            ui.spinner();
                            ui.label("Loading preview…");
                        }
                        Some(Preview::Text(text)) => {
                            if r.file_type() == "Markdown" && self.render_markdown {
                                CommonMarkViewer::new()
                                    .default_implicit_uri_scheme(media::markdown_base(&r.target))
                                    .show(ui, &mut self.markdown, text);
                            } else {
                                ui.add(egui::Label::new(RichText::new(text).monospace()).wrap());
                            }
                        }
                        Some(Preview::Image(_)) => {
                            if let Some(texture) = &self.texture {
                                ui.horizontal(|ui| {
                                    ui.label("Zoom");
                                    ui.add(
                                        egui::Slider::new(&mut self.zoom, 0.25..=3.)
                                            .logarithmic(true),
                                    );
                                    if ui.small_button("Fit").clicked() {
                                        self.zoom = 1.;
                                    }
                                });
                                if ui
                                    .checkbox(&mut self.full_resolution, "Load full resolution")
                                    .changed()
                                {
                                    self.preview_key.clear();
                                    ui.ctx().request_repaint();
                                }
                                if media::image_canvas(
                                    ui,
                                    texture,
                                    self.zoom,
                                    (&p.id, &r.target),
                                    false,
                                    true,
                                )
                                .double_clicked()
                                {
                                    self.toggle_expanded_preview();
                                    ctx.request_repaint();
                                }
                            }
                        }
                        Some(Preview::Unsupported(message)) => {
                            ui.add_space(22.);
                            ui.label(RichText::new(r.file_type()).size(30.).strong());
                            ui.label(message);
                        }
                        Some(Preview::Error(message)) => {
                            ui.add_space(22.);
                            ui.colored_label(Color32::from_rgb(235, 142, 145), message);
                        }
                    });
            });
    }
}
impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "feedr.render_markdown", &self.render_markdown);
        eframe::set_value(storage, "feedr.session_colors", &self.session_colors);
        eframe::set_value(storage, "feedr.session_view", &self.session_view);
        eframe::set_value(storage, "feedr.columns", &self.columns);
        eframe::set_value(
            storage,
            "feedr.show_unavailable",
            &!self.query.available_only,
        );
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        self.artifacts.receive(ctx);
        // Receive decoded images and upload textures on the GUI thread only.
        if let Some(rx) = &self.preview_rx
            && let Ok(preview) = rx.try_recv()
        {
            if let Preview::Image(ref image) = preview {
                self.texture = Some(ctx.load_texture(
                    &self.preview_key,
                    image.clone(),
                    egui::TextureOptions::LINEAR,
                ));
            }
            self.preview = Some(preview);
            self.preview_rx = None;
        }
        self.receive();
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
            self.search_focus = true;
            self.expanded_preview = false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.dismiss_preview();
        }
        if !self.can_expand_preview() {
            self.expanded_preview = false;
        }
        if self.selected.is_none() {
            self.media = None;
            self.media_key.clear();
        }
        if !self.expanded_preview && !ctx.wants_keyboard_input() {
            let down =
                ctx.input(|i| i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::J));
            let up =
                ctx.input(|i| i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::K));
            if down || up {
                self.navigate_artifact(down);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::P))
                && let Some(p) = self
                    .selected
                    .as_ref()
                    .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
            {
                self.edit(p, |u| u.pinned = Some(!p.pinned));
            }
        }
        if self.expanded_preview {
            self.expanded_viewer(ctx);
        } else {
            self.header(ctx);
            self.discovery_settings(ctx);
            self.sidebar(ctx);
            self.detail(ctx);
            self.feed(ctx);
        }
        let summon_enabled = self.hotkey.is_some();
        if self.show_help && !self.expanded_preview {
            egui::Window::new("Publishing to Feedr").open(&mut self.show_help).default_width(570.).show(ctx,|ui|{
            ui.heading("Let your agent post the useful things.");ui.label("Run from an agent's herdr pane for automatic session attribution.");ui.code("feedr post --title 'Report is ready' ./report.md\nfeedr post --title 'Preview the app' http://localhost:3000\nfeedr post --title 'Output folder' ./output");
            ui.add_space(10.);ui.label("For MCP clients, run feedr mcp-config and add the printed configuration. A global MCP client can call feedr_sessions and pass session_id when publishing.");
            ui.label(if summon_enabled {"Ctrl+Alt+F summons Feedr while it is running."} else {"Global summon is unavailable here. Open Feedr from your desktop launcher."});
            ui.separator();ui.label("⌘/Ctrl K — search    ↑/↓ or J/K — select    P — pin    Esc — close preview");ui.label("Images, Markdown, PDF and video open in the sidebar. Video starts paused; press Play for audio and playback.");
        });
        }
        if self.connections && !self.expanded_preview {
            egui::Window::new("Herdr connections")
                .open(&mut self.connections)
                .show(ctx, |ui| {
                    for c in &self.snapshot.connections {
                        ui.label(
                            RichText::new(if c.connected {
                                "Connected"
                            } else {
                                "Unavailable"
                            })
                            .strong(),
                        );
                        ui.label(&c.socket);
                        ui.label(&c.message);
                        ui.separator();
                    }
                    if ui.button("Refresh feed").clicked() {
                        let _ = self.commands.send(Command::Refresh);
                    }
                });
        }
        // Route document links through the same validated resource actions.
        let links = ctx.output_mut(|output| {
            let mut links = Vec::new();
            output.commands.retain(|command| match command {
                egui::OutputCommand::OpenUrl(url) => {
                    links.push(url.url.clone());
                    false
                }
                _ => true,
            });
            links
        });
        for link in links {
            if let Some(post) = self
                .selected
                .as_ref()
                .and_then(|id| self.snapshot.posts.iter().find(|p| &p.id == id))
                .cloned()
            {
                let mut context = post.context.clone();
                if let Some(resource) = post.resources.get(self.resource)
                    && let Some(parent) = Path::new(&resource.target).parent()
                {
                    context.cwd = parent.to_string_lossy().into_owned();
                }
                match Resource::resolve(
                    ResourceInput {
                        uri: link,
                        kind: None,
                    },
                    &context,
                ) {
                    Ok(resource) => self.action(&post, &resource, false),
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
        }
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn load_preview(p: &Post, r: &Resource, image_size: u32) -> Preview {
    if r.kind == ResourceKind::Url {
        return Preview::Unsupported("Open this link in your default browser.".into());
    }
    if !p.context.is_local() {
        return Preview::Unsupported("This resource lives on another machine. Remote retrieval is not available in this build.".into());
    }
    let path = Path::new(&r.target);
    if !path.is_absolute() {
        return Preview::Error("This mention has no resolved location. Its producer must supply an absolute path or working directory.".into());
    }
    if !path.exists() {
        return Preview::Error("The resource is not available at this location. It may still be being created, have moved, or be on an unmounted drive.".into());
    }
    if path.is_dir() {
        return match fs::read_dir(path) {
            Ok(entries) => {
                let mut names: Vec<_> = entries
                    .take(200)
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        format!(
                            "{}{}",
                            e.file_name().to_string_lossy(),
                            if e.path().is_dir() { "/" } else { "" }
                        )
                    })
                    .collect();
                names.sort();
                Preview::Text(names.join("\n"))
            }
            Err(e) => Preview::Error(e.to_string()),
        };
    }
    if r.file_type() == "Image" {
        let result = (|| -> Result<egui::ColorImage> {
            let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(128 * 1024 * 1024);
            limits.max_image_width = Some(16000);
            limits.max_image_height = Some(16000);
            reader.limits(limits);
            let image = reader.decode()?;
            let image = if image.width() > image_size || image.height() > image_size {
                image.thumbnail(image_size, image_size)
            } else {
                image
            }
            .to_rgba8();
            Ok(egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ))
        })();
        return match result {
            Ok(i) => Preview::Image(i),
            Err(e) => Preview::Error(format!("Cannot decode image: {e}")),
        };
    }
    if matches!(r.file_type(), "PDF" | "Video") {
        let cancel = AtomicBool::new(false);
        let result = if r.file_type() == "PDF" {
            crate::media::pdf::render(path, 1, image_size.min(1600), &cancel)
        } else {
            crate::media::poster(path, image_size.min(1280), &cancel)
        };
        return match result {
            Ok(image) => Preview::Image(image),
            Err(error) => Preview::Error(format!("Preview unavailable: {error:#}")),
        };
    }
    if ["Markdown", "File"].contains(&r.file_type()) {
        let result = (|| -> Result<String> {
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take(256 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= 256 * 1024,
                "Text preview is limited to 256 KiB; use Open for this file"
            );
            anyhow::ensure!(
                !bytes.contains(&0),
                "Binary file; use Open in its default application"
            );
            Ok(String::from_utf8(bytes)?)
        })();
        return match result {
            Ok(t) => Preview::Text(t),
            Err(e) => Preview::Unsupported(e.to_string()),
        };
    }
    Preview::Unsupported(format!(
        "Built-in {} preview is planned. Use Open to inspect this resource in its default application.",
        r.file_type()
    ))
}

fn register_summon(ctx: &egui::Context) -> Result<global_hotkey::GlobalHotKeyManager> {
    use global_hotkey::{
        GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
        hotkey::{Code, HotKey, Modifiers},
    };
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyF);
    manager.register(hotkey)?;
    let ctx = ctx.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == hotkey.id() && event.state == HotKeyState::Pressed {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }));
    Ok(manager)
}
fn ellipsize(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.into()
    } else {
        format!(
            "{}…",
            text.chars().take(max.saturating_sub(1)).collect::<String>()
        )
    }
}
fn age(timestamp: i64) -> String {
    let seconds = ((now() - timestamp) / 1000).max(0);
    if seconds < 60 {
        "Just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}

// A fixed, fully clickable one-line surface. Unlike add_sized(Label), the
// text stays left-aligned and the entire allocated tab responds to a gesture.
fn line_action(
    ui: &mut egui::Ui,
    text: RichText,
    width: f32,
    height: f32,
    inset: f32,
    sense: egui::Sense,
) -> egui::Response {
    let text: egui::WidgetText = text.into();
    let label = text.text().to_owned();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width.max(0.), height), sense);
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (width - inset).max(0.),
        egui::TextStyle::Button,
    );
    let position = egui::pos2(rect.left() + inset, rect.center().y - galley.size().y / 2.);
    ui.painter()
        .with_clip_rect(rect.intersect(ui.clip_rect()))
        .galley(position, galley, INK);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
    });
    response
}
fn quiet_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.).color(MUTED))
            .small()
            .frame(false),
    )
}

fn opens_in_sidebar(resource: &Resource) -> bool {
    matches!(resource.file_type(), "Image" | "Markdown" | "PDF" | "Video")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (commands, _) = mpsc::channel();
        let (_, events) = mpsc::channel();
        App {
            artifacts: artifact::Artifacts::new(),
            hotkey: None,
            snapshot: Snapshot::default(),
            columns: ColumnLayout::default(),
            feeds: HashMap::from([(FeedTarget::All, Feed::default())]),
            reveal_column: None,
            dragging_column: None,
            query: FeedQuery::default(),
            commands,
            events,
            stop: Arc::new(AtomicBool::new(false)),
            selected: None,
            reveal_session: None,
            reveal_artifact: false,
            session_colors: HashMap::new(),
            session_view: session_list::View::default(),
            resource: 0,
            error: None,
            search_focus: false,
            preview_key: String::new(),
            preview_rx: None,
            preview_active: Arc::new(AtomicUsize::new(0)),
            preview: None,
            texture: None,
            markdown: CommonMarkCache::default(),
            render_markdown: true,
            media: None,
            media_key: String::new(),
            zoom: 1.,
            full_resolution: false,
            expanded_preview: false,
            expanded_zoom: 1.,
            show_help: false,
            connections: false,
            show_discovery: false,
        }
    }
    fn post() -> Post {
        Post {
            discovery: None,
            id: "post-with-multiple-attachments".into(),
            session_id: "session".into(),
            title: "Deliverables".into(),
            body: String::new(),
            resources: [
                ("/missing/folder", ResourceKind::Folder),
                ("/missing/image.png", ResourceKind::File),
                ("/missing/report.md", ResourceKind::File),
                ("/missing/report.pdf", ResourceKind::File),
                ("/missing/video.mp4", ResourceKind::File),
            ]
            .into_iter()
            .map(|(target, kind)| Resource {
                original: target.into(),
                target: target.into(),
                name: target.rsplit('/').next().unwrap().into(),
                kind,
            })
            .collect(),
            context: Context::default(),
            readiness: "ready".into(),
            revision: 1,
            created_at: now(),
            updated_at: now(),
            pinned: false,
            archived: false,
            read: true,
        }
    }
    #[test]
    fn clicking_images_and_markdown_selects_the_exact_sidebar_attachment() {
        let mut app = app();
        let post = post();
        for index in [1, 2, 3, 4] {
            app.activate_artifact(&post, index);
            assert_eq!(app.selected.as_deref(), Some(post.id.as_str()));
            assert_eq!(app.resource, index);
            // These paths do not exist: routing to the external opener would
            // produce an error instead of selecting the sidebar attachment.
            assert!(app.error.is_none());
        }
    }
    #[test]
    fn changing_attachments_keeps_the_markdown_source_preference() {
        let mut app = app();
        let post = post();
        app.render_markdown = false;
        app.activate_artifact(&post, 2);
        app.activate_artifact(&post, 1);
        app.activate_artifact(&post, 2);
        assert!(!app.render_markdown);
        assert_eq!(app.resource, 2);
    }
    #[test]
    fn expanded_preview_returns_to_the_same_attachment_before_closing() {
        let mut app = app();
        let post = post();
        app.snapshot.posts = vec![post.clone()];
        for index in [1, 4] {
            app.select_resource(&post, index);
            app.zoom = 2.;
            app.media_key = "same-player-key".into();
            app.toggle_expanded_preview();
            assert!(app.expanded_preview);
            app.dismiss_preview();
            assert!(!app.expanded_preview);
            assert_eq!(app.selected.as_deref(), Some(post.id.as_str()));
            assert_eq!(app.resource, index);
            assert_eq!(app.zoom, 2.);
            assert_eq!(app.media_key, "same-player-key");
            app.dismiss_preview();
            assert!(app.selected.is_none());
            assert!(app.media_key.is_empty());
        }
        app.select_resource(&post, 2);
        app.toggle_expanded_preview();
        assert!(!app.expanded_preview);
    }
    #[test]
    fn double_clicking_the_sidebar_image_fills_the_window_and_escape_restores_it() {
        let mut app = app();
        let post = post();
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![post.clone()];
        app.merge_posts();
        app.select_resource(&post, 1);
        let resource = &post.resources[1];
        let ctx = egui::Context::default();
        let image = egui::ColorImage::filled([64, 32], BLUE);
        app.texture =
            Some(ctx.load_texture("test-image", image.clone(), egui::TextureOptions::LINEAR));
        let texture_id = app.texture.as_ref().unwrap().id();
        app.preview = Some(Preview::Image(image));
        app.preview_key = format!(
            "{}:{}:{:?}",
            post.context.host, resource.target, None::<std::time::SystemTime>
        );
        let frame = |app: &mut App, time, events| {
            let _ = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400., 1000.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| eframe::App::update(app, ctx, &mut eframe::Frame::_new_kittest()),
            );
        };
        frame(&mut app, 0., vec![]);
        let rect = ctx.data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::new(("image-preview-rect", false)))
                .unwrap()
        });
        let pos = rect.center();
        for (time, pressed) in [(0.1, true), (0.11, false), (0.2, true), (0.21, false)] {
            frame(
                &mut app,
                time,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(app.expanded_preview);
        frame(&mut app, 0.22, vec![]);
        let expanded = ctx.data(|data| {
            data.get_temp::<egui::Rect>(egui::Id::new(("image-preview-rect", true)))
                .unwrap()
        });
        assert!(expanded.width() > rect.width() * 2.);
        assert!(expanded.height() > 620.);
        assert_eq!(app.texture.as_ref().unwrap().id(), texture_id);
        frame(
            &mut app,
            0.3,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!app.expanded_preview);
        assert_eq!(app.selected.as_deref(), Some(post.id.as_str()));
        assert_eq!(app.resource, 1);
    }
    #[test]
    fn dense_row_clicks_open_exact_attachments_and_focus_the_clicked_feed() {
        let mut app = app();
        let post = post();
        let target = FeedTarget::Session(post.session_id.clone());
        let ctx = egui::Context::default();
        let draw = |app: &mut App, events| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500., 400.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.card(ui, &post, &target));
                },
            );
        };
        draw(&mut app, vec![]);
        for index in [1, 2, 3, 4, 1] {
            app.columns.active = Some(FeedTarget::All);
            let row = ctx.data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("dense-resource-row", &post.id, index)))
                    .unwrap()
            });
            let at = row.left_center() + egui::vec2(16., 0.);
            draw(
                &mut app,
                vec![
                    egui::Event::PointerMoved(at),
                    egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            draw(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(app.selected.as_deref(), Some(post.id.as_str()));
            assert_eq!(app.resource, index);
            assert_eq!(app.columns.active.as_ref(), Some(&target));
            assert!(app.error.is_none());
        }
    }
    #[test]
    fn dense_keyboard_navigation_visits_attachments_before_the_next_post() {
        let mut app = app();
        let mut first = post();
        first.resources.truncate(2);
        let mut second = post();
        second.id = "next-post".into();
        second.resources = vec![second.resources[2].clone()];
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts =
            vec![first.clone(), second.clone()];
        app.merge_posts();
        for (id, index) in [
            (first.id.as_str(), 0),
            (first.id.as_str(), 1),
            (second.id.as_str(), 0),
            (second.id.as_str(), 0),
        ] {
            app.navigate_artifact(true);
            assert_eq!(app.selected.as_deref(), Some(id));
            assert_eq!(app.resource, index);
            assert!(app.reveal_artifact);
        }
        app.navigate_artifact(false);
        assert_eq!(app.selected.as_deref(), Some(first.id.as_str()));
        assert_eq!(app.resource, 1);
    }

    #[test]
    fn availability_changes_preserve_the_selected_resource_identity() {
        let mut app = app();
        let mut post = post();
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![post.clone()];
        app.merge_posts();
        app.select_resource(&post, 2);
        let selected = post.resources[2].target.clone();
        post.resources.remove(0);
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![post.clone()];
        app.merge_posts();
        assert_eq!(app.resource, 1);
        assert_eq!(
            app.snapshot.posts[0].resources[app.resource].target,
            selected
        );
        post.resources.remove(1);
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![post];
        app.merge_posts();
        assert!(app.selected.is_none());
    }
    #[test]
    fn full_resolution_preserves_native_image_size_instead_of_upscaling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        image::RgbaImage::from_pixel(64, 32, image::Rgba([20, 40, 60, 255]))
            .save(&path)
            .unwrap();
        let mut post = post();
        post.resources[1].target = path.to_string_lossy().into_owned();
        let Preview::Image(full) = load_preview(&post, &post.resources[1], 16000) else {
            panic!("Full image did not load")
        };
        assert_eq!(full.size, [64, 32]);
        let Preview::Image(thumbnail) = load_preview(&post, &post.resources[1], 16) else {
            panic!("Thumbnail did not load")
        };
        assert_eq!(thumbnail.size, [16, 8]);
    }
    #[test]
    fn card_selection_tracks_its_session_without_changing_the_feed_filter() {
        let mut app = app();
        let first = post();
        let mut second = post();
        second.id = "another-post".into();
        second.session_id = "another-session".into();
        app.snapshot.posts = vec![first.clone(), second.clone()];

        app.activate_artifact(&first, 1);
        assert_eq!(app.active_session(), Some(first.session_id.as_str()));
        assert_eq!(app.reveal_session.take(), Some(first.session_id.clone()));
        // Changing attachments within the same session must not yank the
        // user's independently scrolled session list back into position.
        app.activate_artifact(&first, 2);
        assert!(app.reveal_session.is_none());

        app.select(second.id.clone());
        assert_eq!(app.active_session(), Some(second.session_id.as_str()));
        assert_eq!(
            app.reveal_session.as_deref(),
            Some(second.session_id.as_str())
        );
        assert!(app.query.session_id.is_none());
        assert_eq!(app.snapshot.posts.len(), 2);

        app.selected = None;
        assert!(app.active_session().is_none());
    }
    #[test]
    fn saved_sidebar_preferences_restore_and_unknown_colors_fall_back() {
        #[derive(Default)]
        struct Storage(HashMap<String, String>);
        impl eframe::Storage for Storage {
            fn get_string(&self, key: &str) -> Option<String> {
                self.0.get(key).cloned()
            }
            fn set_string(&mut self, key: &str, value: String) {
                self.0.insert(key.into(), value);
            }
            fn flush(&mut self) {}
        }

        let mut app = app();
        app.session_view = session_list::View::Priority;
        app.columns.open(FeedTarget::Session("session".into()));
        app.columns.close(&FeedTarget::All);
        app.session_colors.insert("session".into(), "iris".into());
        app.session_colors
            .insert("other".into(), "unknown-color".into());
        let mut storage = Storage::default();
        eframe::App::save(&mut app, &mut storage);
        let restored: HashMap<String, String> =
            eframe::get_value(&storage, "feedr.session_colors").unwrap();
        assert!(matches!(
            eframe::get_value::<session_list::View>(&storage, "feedr.session_view"),
            Some(session_list::View::Priority)
        ));
        let columns: ColumnLayout = eframe::get_value(&storage, "feedr.columns").unwrap();
        assert_eq!(columns.order, [FeedTarget::Session("session".into())]);
        assert_eq!(session_color::resolve("session", &restored).id, "iris");
        assert_eq!(
            session_color::resolve("other", &restored).id,
            session_color::resolve("other", &HashMap::new()).id,
        );
        // An unrelated override cannot reshuffle another session's default.
        assert_eq!(
            session_color::resolve("untouched", &restored).id,
            session_color::resolve("untouched", &HashMap::new()).id,
        );
    }

    #[test]
    fn session_columns_keep_independent_results_and_ignore_stale_filter_responses() {
        let mut app = app();
        let target = FeedTarget::Session("quiet-session".into());
        app.open_feed(target.clone());
        let mut quiet = post();
        quiet.id = "quiet-post".into();
        quiet.session_id = "quiet-session".into();
        let busy = post();
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![busy];
        let (tx, rx) = mpsc::channel();
        app.events = rx;
        let query = app.feeds[&target].query(&app.query, &target);
        let mut stale = query.clone();
        stale.search = "old search".into();
        tx.send(Event::Snapshot(
            target.clone(),
            query,
            Box::new(Snapshot {
                posts: vec![quiet.clone()],
                ..Snapshot::default()
            }),
        ))
        .unwrap();
        tx.send(Event::Snapshot(target.clone(), stale, Box::default()))
            .unwrap();
        app.receive();
        assert_eq!(app.feeds[&target].snapshot.posts[0].id, quiet.id);
        assert_eq!(app.snapshot.posts.len(), 2);
        app.select(quiet.id.clone());
        app.close_feed(&FeedTarget::All);
        assert_eq!(app.selected.as_deref(), Some("quiet-post"));
        assert_eq!(app.snapshot.posts.len(), 1);
        app.close_feed(&target);
        assert!(app.selected.is_none());
        assert!(app.snapshot.posts.is_empty());
    }

    #[test]
    fn incoming_posts_pause_only_the_column_being_read() {
        let mut app = app();
        let target = FeedTarget::Session("quiet-session".into());
        app.open_feed(target.clone());
        let mut quiet = post();
        quiet.id = "quiet-post".into();
        quiet.session_id = "quiet-session".into();
        app.feeds.get_mut(&target).unwrap().snapshot.posts = vec![quiet.clone()];
        app.feeds.get_mut(&FeedTarget::All).unwrap().snapshot.posts = vec![post()];
        app.merge_posts();
        app.select(quiet.id.clone());
        let mut added = quiet.clone();
        added.id = "new-quiet-post".into();
        quiet.read = true;
        quiet.revision += 1;
        let (tx, rx) = mpsc::channel();
        app.events = rx;
        tx.send(Event::Snapshot(
            target.clone(),
            app.feeds[&target].query(&app.query, &target),
            Box::new(Snapshot {
                total: 2,
                posts: vec![added.clone(), quiet.clone()],
                ..Snapshot::default()
            }),
        ))
        .unwrap();
        tx.send(Event::Snapshot(
            FeedTarget::All,
            app.feeds[&FeedTarget::All].query(&app.query, &FeedTarget::All),
            Box::new(Snapshot {
                total: 2,
                posts: vec![added.clone(), post()],
                ..Snapshot::default()
            }),
        ))
        .unwrap();
        app.receive();
        assert!(app.feeds[&target].pending.is_some());
        assert_eq!(app.feeds[&target].snapshot.posts.len(), 1);
        assert!(app.feeds[&target].snapshot.posts[0].read);
        assert!(app.feeds[&FeedTarget::All].pending.is_none());
        assert_eq!(app.feeds[&FeedTarget::All].snapshot.posts[0].id, added.id);
        assert_eq!(app.selected.as_deref(), Some("quiet-post"));
    }

    #[test]
    fn dragging_the_tab_handle_reorders_the_rendered_columns() {
        for compressed in [false, true] {
            let mut app = app();
            let session = FeedTarget::Session("session".into());
            app.open_feed(session.clone());
            app.reveal_column = None;
            let ctx = egui::Context::default();
            let mut frame = |events| {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1100., 800.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ctx| app.feed(ctx),
                );
            };
            frame(vec![]);
            frame(vec![]);
            let rect = |target: &FeedTarget| {
                ctx.data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new(("feed-column-rect", target)))
                        .unwrap()
                })
            };
            let from = rect(&session).center();
            let to = rect(&FeedTarget::All).left_center() + egui::vec2(70., 0.);
            let mut press = vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let release = vec![
                egui::Event::PointerMoved(to),
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            if compressed {
                press.extend(release);
                frame(press);
            } else {
                frame(press);
                frame(release);
            }
            assert_eq!(
                app.columns.order,
                [session, FeedTarget::All],
                "compressed={compressed}"
            );
        }
    }
}
