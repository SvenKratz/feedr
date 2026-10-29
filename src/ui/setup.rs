use super::{BLUE, MUTED};
use crate::{
    agent_setup::{self, Agent, Profile, Status},
    cli_install,
    setup::McpSetup,
};
use eframe::egui::{self, RichText};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) struct Setup {
    data: PathBuf,
    exe: PathBuf,
    first_launch: bool,
    was_open: bool,
    selected: [bool; 2],
    reminders: bool,
    statuses: Vec<Status>,
    events: Option<mpsc::Receiver<AgentEvent>>,
    detected: bool,
    message: Option<String>,
    mcp: Result<McpSetup, String>,
    copied: Option<(usize, Instant)>,
    pub cli_open: bool,
    installing: Option<mpsc::Receiver<Result<cli_install::Installed, String>>>,
    installed: Option<Result<cli_install::Installed, String>>,
}
enum AgentEvent {
    Status(Vec<Status>),
    Applied(Vec<(Agent, Result<Status, String>)>),
    Test(Result<(), String>),
}
impl Setup {
    pub fn new(data: PathBuf) -> Self {
        Self {
            exe: std::env::current_exe().unwrap_or_default(),
            first_launch: !agent_setup::completed(&data),
            data: data.clone(),
            was_open: false,
            selected: [false; 2],
            reminders: true,
            statuses: Vec::new(),
            events: None,
            detected: false,
            message: None,
            mcp: std::env::current_exe()
                .map(|exe| McpSetup::new(&exe, &data))
                .map_err(|e| e.to_string()),
            copied: None,
            cli_open: false,
            installing: None,
            installed: None,
        }
    }
    pub fn show(&mut self, ctx: &egui::Context, open: &mut bool) {
        if self.first_launch {
            *open = true;
            self.first_launch = false;
        }
        let event = self.events.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.message =
                    Some("Setup was interrupted. Reopen Agent setup to try again.".into());
                Some(AgentEvent::Status(self.statuses.clone()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        });
        if let Some(event) = event {
            self.events = None;
            match event {
                AgentEvent::Status(statuses) => {
                    if !self.detected {
                        for (index, agent) in Agent::ALL.iter().enumerate() {
                            self.selected[index] =
                                statuses.iter().any(|s| s.agent == *agent && s.detected);
                        }
                        self.detected = true;
                    }
                    self.statuses = statuses;
                }
                AgentEvent::Applied(results) => {
                    let mut failed = false;
                    for (agent, result) in results {
                        match result {
                            Ok(status) => {
                                self.statuses.retain(|s| s.agent != agent);
                                self.statuses.push(status);
                            }
                            Err(error) => {
                                failed = true;
                                if let Some(status) =
                                    self.statuses.iter_mut().find(|s| s.agent == agent)
                                {
                                    status.issue = Some(error);
                                }
                            }
                        }
                    }
                    self.message = Some(
                        if failed {
                            "Some settings could not be saved. See the agent details below."
                        } else {
                            "Settings saved. Restart your connected agents to load them."
                        }
                        .into(),
                    );
                }
                AgentEvent::Test(result) => {
                    self.message = Some(match result {
                        Ok(()) => {
                            "Test share delivered. Close setup to see it in your feed.".into()
                        }
                        Err(error) => format!("Could not send the test share: {error}"),
                    })
                }
            }
        }
        if *open && !self.was_open {
            self.refresh(ctx);
        }
        self.was_open = *open;
        if *open {
            let mut window_open = true;
            let mut close_requested = false;
            egui::Window::new("Set up Feedr")
                .id(egui::Id::new("feedr-agent-setup-v2"))
                .open(&mut window_open)
                .default_width(640.)
                .default_height(460.)
                .max_width(780.)
                .show(ctx, |ui| {
                    ui.label("Connect an agent to receive finished reports, media, builds and preview links.");
                    ui.label(RichText::new("Works across projects. Herdr and discovery are optional.").small().color(MUTED));
                    ui.add_space(10.);
                    egui::ScrollArea::vertical().max_height((ctx.available_rect().height() - 210.).clamp(160., 560.)).show(ui, |ui| {
                        self.agent_rows(ui);
                        ui.add_space(8.);
                        ui.add_enabled_ui(self.events.is_none(), |ui| {
                            ui.checkbox(&mut self.reminders, "Remind agents about missed shares");
                        });
                        if self.reminders && self.selected[1] {
                            ui.label(RichText::new("Codex: after restarting, use /hooks to review and trust Feedr's reminders.").small().color(MUTED));
                        }
                        ui.collapsing("What agents will share", |ui| {
                            ui.label("Finished documents, images, video, audio, builds, output folders and preview links. Routine source edits, logs and scratch files stay out.");
                            ui.label("Feedr adds an MCP connection and publishing instructions to the selected clients. The optional completion check reminds once about missed shares and lets tasks finish if Feedr is offline.");
                        });
                        ui.collapsing("Files to update", |ui| {
                            for status in &self.statuses {
                                if self.selected[if status.agent == Agent::Claude { 0 } else { 1 }] {
                                    ui.label(RichText::new(status.agent.name()).strong());
                                    for file in &status.files { ui.label(RichText::new(file.display().to_string()).small().color(MUTED)); }
                                }
                            }
                            ui.label(RichText::new("Existing settings are preserved. Backups stay in Feedr's private data folder.").small().color(MUTED));
                        });
                        ui.collapsing("Manual MCP configuration", |ui| match &self.mcp {
                            Ok(mcp) => {
                                snippet(ui, "MCP configuration", &mcp.json, 0, &mut self.copied);
                                snippet(ui, "Claude Code", &mcp.claude, 1, &mut self.copied);
                                snippet(ui, "Codex", &mcp.codex, 2, &mut self.copied);
                            }
                            Err(error) => { ui.label(error); }
                        });
                    });
                    if let Some(message) = &self.message { ui.add_space(8.); ui.label(RichText::new(message).color(BLUE)); }
                    ui.add_space(10.);
                    ui.horizontal_wrapped(|ui| {
                        let busy = self.events.is_some();
                        if ui.add_enabled(!busy && self.detected && self.selected.iter().any(|selected| *selected), egui::Button::new("Enable publishing")).clicked() {
                            self.apply(ctx, None);
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Send test share")).clicked() {
                            let data = self.data.clone();
                            self.work(ctx, move || AgentEvent::Test(agent_setup::test_post(&data).map(|_| ()).map_err(|e| format!("{e:#}"))));
                        }
                        if busy { ui.spinner(); }
                        if ui.add_enabled(!busy, egui::Button::new(if self.statuses.iter().any(Status::configured) { "Done" } else { "Do this later" })).clicked() { close_requested = true; }
                    });
                });
            if !window_open || close_requested {
                match agent_setup::finish(&self.data) {
                    Ok(()) => {
                        *open = false;
                        self.was_open = false;
                    }
                    Err(error) => {
                        self.message = Some(format!("Could not save setup preferences: {error}"));
                    }
                }
            }
        }
        if let Some(rx) = &self.installing
            && let Ok(result) = rx.try_recv()
        {
            self.installed = Some(result);
            self.installing = None;
        }
        if self.cli_open {
            let mut cli_open = true;
            egui::Window::new("Install CLI").id(egui::Id::new("feedr-install-cli"))
                .open(&mut cli_open).default_width(460.).show(ctx, |ui| {
                    ui.label("Use feedr from Terminal and agent shells.");
                    ui.add_space(4.);
                    ui.label(RichText::new("Links ~/.local/bin/feedr to this app and configures PATH for new terminal windows.").small().color(MUTED));
                    ui.add_space(10.);
                    if let Some(result) = &self.installed {
                        match result {
                            Ok(result) => {
                                ui.colored_label(BLUE, "CLI installed");
                                ui.monospace(result.command.display().to_string());
                                ui.label(&result.next_step);
                            }
                            Err(error) => { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                        }
                    }
                    if self.installing.is_some() {
                        ui.horizontal(|ui| { ui.spinner(); ui.label("Installing…"); });
                    } else if self.installed.as_ref().is_none_or(|r| r.is_err()) && ui.button("Install CLI").clicked() {
                        let (tx, rx) = mpsc::channel();
                        self.installing = Some(rx);
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let _ = tx.send(cli_install::install(None, true).map_err(|e| format!("{e:#}")));
                            ctx.request_repaint();
                        });
                    }
                });
            self.cli_open = cli_open;
        }
    }
    fn work(&mut self, ctx: &egui::Context, task: impl FnOnce() -> AgentEvent + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        self.events = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task());
            ctx.request_repaint();
        });
    }
    #[cfg(test)]
    pub fn for_test(data: PathBuf) -> Self {
        let mut setup = Self::new(data);
        setup.first_launch = false;
        setup
    }
    fn refresh(&mut self, ctx: &egui::Context) {
        let exe = self.exe.clone();
        let data = self.data.clone();
        self.work(ctx, move || {
            AgentEvent::Status(
                Agent::ALL
                    .into_iter()
                    .map(|agent| match Profile::locate(agent, None) {
                        Ok(profile) => agent_setup::inspect(&profile, &exe, &data),
                        Err(error) => Status {
                            agent,
                            detected: false,
                            connection: false,
                            instructions: false,
                            reminders: false,
                            files: Vec::new(),
                            issue: Some(format!("{error:#}")),
                        },
                    })
                    .collect(),
            )
        });
    }
    fn apply(&mut self, ctx: &egui::Context, remove: Option<Agent>) {
        let agents: Vec<_> = Agent::ALL
            .into_iter()
            .enumerate()
            .filter(|(i, agent)| remove.map_or(self.selected[*i], |which| *agent == which))
            .map(|(_, agent)| agent)
            .collect();
        let exe = self.exe.clone();
        let data = self.data.clone();
        let reminders = self.reminders;
        self.work(ctx, move || {
            AgentEvent::Applied(
                agents
                    .into_iter()
                    .map(|agent| {
                        let result = Profile::locate(agent, None)
                            .and_then(|profile| {
                                agent_setup::configure(
                                    &profile,
                                    &exe,
                                    &data,
                                    reminders,
                                    remove.is_none(),
                                )
                            })
                            .map_err(|e| format!("{e:#}"));
                        (agent, result)
                    })
                    .collect(),
            )
        });
    }
    fn agent_rows(&mut self, ui: &mut egui::Ui) {
        let busy = self.events.is_some();
        let mut remove = None;
        for (index, agent) in Agent::ALL.into_iter().enumerate() {
            let status = self.statuses.iter().find(|status| status.agent == agent);
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !busy && self.detected,
                    egui::Checkbox::new(
                        &mut self.selected[index],
                        RichText::new(agent.name()).strong(),
                    ),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if status.is_some_and(Status::configured)
                        && ui
                            .add_enabled(!busy, egui::Button::new("Disconnect").small())
                            .clicked()
                    {
                        remove = Some(agent);
                    }
                    ui.label(
                        RichText::new(match status {
                            Some(s) if s.configured() => "Configured",
                            Some(s) if s.issue.is_some() => "Needs attention",
                            Some(s) if s.detected => "Detected",
                            Some(_) => "Not detected",
                            None => "Checking…",
                        })
                        .small()
                        .color(
                            if status.is_some_and(Status::configured) {
                                BLUE
                            } else {
                                MUTED
                            },
                        ),
                    );
                });
            });
            if let Some(status) = status {
                if let Some(error) = &status.issue {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if status.configured() {
                    ui.label(
                        RichText::new(if status.reminders {
                            "MCP connection, publishing rule and completion reminders configured"
                        } else {
                            "MCP connection and publishing rule configured"
                        })
                        .small()
                        .color(MUTED),
                    );
                }
            }
            ui.add_space(6.);
        }
        if let Some(agent) = remove {
            self.apply(ui.ctx(), Some(agent));
        }
    }
}

fn snippet(
    ui: &mut egui::Ui,
    label: &str,
    text: &str,
    id: usize,
    copied: &mut Option<(usize, Instant)>,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).strong().color(BLUE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let done = copied
                .is_some_and(|(which, at)| which == id && at.elapsed() < Duration::from_secs(2));
            if ui
                .small_button(if done { "Copied" } else { "Copy" })
                .clicked()
            {
                ui.ctx().copy_text(text.to_owned());
                *copied = Some((id, Instant::now()));
            }
        });
    });
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(10, 20, 28))
        .corner_radius(4)
        .inner_margin(9.)
        .show(ui, |ui| {
            let rows = text.lines().count();
            let mut text = text;
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .font(egui::FontId::monospace(12.))
                    .desired_width(f32::INFINITY)
                    .desired_rows(rows)
                    .frame(false),
            );
        });
}
