use super::{BLUE, MUTED};
use crate::{cli_install, setup::McpSetup};
use eframe::egui::{self, RichText};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) struct Setup {
    mcp: Result<McpSetup, String>,
    copied: Option<(usize, Instant)>,
    pub cli_open: bool,
    installing: Option<mpsc::Receiver<Result<cli_install::Installed, String>>>,
    installed: Option<Result<cli_install::Installed, String>>,
}
impl Setup {
    pub fn new(data: PathBuf) -> Self {
        Self {
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
        if *open {
            egui::Window::new("Connect your agent")
                .id(egui::Id::new("feedr-mcp-setup"))
                .open(open)
                .default_width(610.)
                .default_height(560.)
                .max_width(760.)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(650.)
                        .show(ui, |ui| match &self.mcp {
                            Ok(mcp) => {
                                snippet(ui, "MCP configuration", &mcp.json, 0, &mut self.copied);
                                ui.add_space(8.);
                                snippet(ui, "Claude Code", &mcp.claude, 1, &mut self.copied);
                                ui.add_space(8.);
                                snippet(ui, "Codex", &mcp.codex, 2, &mut self.copied);
                                ui.add_space(6.);
                                ui.label(
                                    RichText::new("Restart the agent after adding Feedr.")
                                        .small()
                                        .color(MUTED),
                                );
                            }
                            Err(error) => {
                                ui.label(error);
                            }
                        });
                });
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
