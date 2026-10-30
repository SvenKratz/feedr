//! Match herdr's native workspace order and status/transition priority.
use super::{BLUE, INK, MUTED, PANEL, session_color::SessionColor};
use crate::model::Session;
use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum View {
    #[default]
    Grouped,
    Priority,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Group<'a> {
    Workspace(&'a str, Option<&'a str>, Option<&'a str>),
    Saved,
}

pub(super) fn group(session: &Session) -> Group<'_> {
    if session.online {
        Group::Workspace(
            &session.context.host,
            session.context.herdr_socket.as_deref(),
            session.context.workspace_id.as_deref(),
        )
    } else {
        Group::Saved
    }
}

pub(super) fn group_label(session: &Session) -> &str {
    if session.online {
        session
            .workspace_label
            .as_deref()
            .or(session.context.workspace_id.as_deref())
            .unwrap_or("Other sessions")
    } else {
        "Saved sessions"
    }
}

pub(super) fn status_label(session: &Session) -> &str {
    if session.online {
        match session.status.as_str() {
            "blocked" => "Blocked",
            "done" => "Done",
            "working" => "Working",
            "idle" => "Idle",
            _ => "Unknown",
        }
    } else {
        "Saved"
    }
}

pub(super) fn name(session: &Session) -> &str {
    session
        .display_name
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or(session
            .workspace_label
            .as_deref()
            .filter(|s| !s.trim().is_empty()))
        .unwrap_or(&session.context.label)
}

pub(super) fn agent(session: &Session) -> &str {
    session
        .display_agent
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&session.context.agent)
}

pub(super) fn recency(timestamp: Option<i64>, current: i64) -> String {
    let Some(timestamp) = timestamp.filter(|t| *t > 0) else {
        return "—".into();
    };
    let minutes = current.saturating_sub(timestamp).max(0) / 60_000;
    match minutes {
        0 => "now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

pub(super) const ROW_HEIGHT: f32 = 36.;

/// Fixed icon and text columns: neither long names nor a large font can wrap a row.
pub(super) fn row(
    ui: &mut egui::Ui,
    session: &Session,
    color: SessionColor,
    selected: bool,
    current: i64,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let fill = if selected {
        color.highlighted()
    } else if response.hovered() {
        color.sidebar()
    } else {
        color.sidebar().lerp_to_gamma(PANEL, 0.72)
    };
    painter.rect_filled(rect, 3., fill);
    if selected || response.has_focus() {
        painter.rect_stroke(
            rect.shrink(0.5),
            3.,
            egui::Stroke::new(1., color.accent),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            egui::Rect::from_min_max(
                rect.min + egui::vec2(0., 4.),
                egui::pos2(rect.left() + 2., rect.bottom() - 4.),
            ),
            1.,
            color.accent,
        );
    }
    let center = rect.min + egui::vec2(11., 11.);
    let status = if session.online {
        session.status.as_str()
    } else {
        "offline"
    };
    let ink = match status {
        "working" => BLUE,
        "done" | "idle" => Color32::from_rgb(147, 182, 150),
        "blocked" => Color32::from_rgb(235, 166, 142),
        _ => MUTED,
    };
    let stroke = egui::Stroke::new(1.4, ink);
    match status {
        "idle" => {
            painter.circle_stroke(center, 3.5, stroke);
        }
        "working" => {
            let mut half = vec![center];
            half.extend((0..=12).map(|i| {
                let angle = std::f32::consts::PI * (i as f32 / 12. - 0.5);
                center + egui::vec2(angle.cos(), angle.sin()) * 3.5
            }));
            painter.add(egui::Shape::convex_polygon(half, ink, egui::Stroke::NONE));
            painter.circle_stroke(center, 3.5, stroke);
        }
        "done" => {
            painter.line_segment(
                [center + egui::vec2(-3., 0.), center + egui::vec2(-1., 2.4)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(-1., 2.4), center + egui::vec2(3.5, -3.)],
                stroke,
            );
        }
        "blocked" => {
            painter.line_segment(
                [center - egui::vec2(3., 3.), center + egui::vec2(3., 3.)],
                stroke,
            );
            painter.line_segment(
                [center + egui::vec2(-3., 3.), center + egui::vec2(3., -3.)],
                stroke,
            );
        }
        _ => {
            painter.circle_filled(center, 1.6, ink);
        }
    }
    let left = rect.left() + 25.;
    let right = rect.right() - 6.;
    let title = egui::WidgetText::from(
        RichText::new(
            name(session)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
        .size(13.)
        .strong()
        .color(INK),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (right - left).max(0.),
        egui::TextStyle::Body,
    );
    painter.galley(egui::pos2(left, rect.top() + 2.), title, INK);
    let age = recency(session.last_activity_at, current);
    let time = painter.layout_no_wrap(age.clone(), egui::FontId::monospace(11.), MUTED);
    let time_left = right - time.size().x;
    let secondary = egui::WidgetText::from(
        RichText::new(
            agent(session)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
        .size(11.)
        .color(MUTED),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (time_left - left - 8.).max(0.),
        egui::TextStyle::Body,
    );
    painter.galley(egui::pos2(left, rect.top() + 20.), secondary, MUTED);
    painter.galley(egui::pos2(time_left, rect.top() + 20.), time, MUTED);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!(
                "{}: {} ({}) · {}",
                status_label(session),
                name(session),
                agent(session),
                age
            ),
        )
    });
    response
}

fn status_priority(session: &Session) -> u8 {
    match session.status.as_str() {
        "blocked" => 4,
        "done" => 3,
        "working" => 2,
        "idle" => 1,
        _ => 0,
    }
}

fn grouped_order(a: &Session, b: &Session) -> Ordering {
    if !a.online && !b.online {
        return a.context.label.cmp(&b.context.label).then(a.id.cmp(&b.id));
    }
    a.context
        .host
        .cmp(&b.context.host)
        .then(a.context.herdr_socket.cmp(&b.context.herdr_socket))
        .then(a.workspace_order.cmp(&b.workspace_order))
        .then(a.context.workspace_id.cmp(&b.context.workspace_id))
        .then(a.pane_order.cmp(&b.pane_order))
        .then(a.context.label.cmp(&b.context.label))
        .then(a.id.cmp(&b.id))
}

pub(super) fn sort(sessions: &mut [Session], view: View) {
    sessions.sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then_with(|| {
                if view == View::Priority && a.online && b.online {
                    status_priority(b)
                        .cmp(&status_priority(a))
                        .then(b.state_change_seq.cmp(&a.state_change_seq))
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| grouped_order(a, b))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Context;

    fn session(id: &str, status: &str, sequence: u64, workspace: usize, pane: usize) -> Session {
        Session {
            id: id.into(),
            context: Context {
                workspace_id: Some(format!("workspace-{workspace}")),
                herdr_socket: Some("/herdr/socket".into()),
                label: id.into(),
                ..Context::default()
            },
            status: status.into(),
            online: true,
            terminal_id: id.into(),
            workspace_label: Some(format!("Project {workspace}")),
            workspace_order: workspace,
            pane_order: pane,
            state_change_seq: sequence,
            display_name: None,
            display_agent: None,
            last_activity_at: None,
        }
    }

    #[test]
    fn priority_matches_herdr_and_keeps_offline_sessions_last() {
        let mut saved = session("saved", "blocked", 100, 0, 0);
        saved.online = false;
        let mut sessions = vec![
            session("idle", "idle", 99, 0, 0),
            session("working", "working", 1, 0, 1),
            saved,
            session("blocked-old", "blocked", 2, 0, 2),
            session("unknown", "future-status", 999, 0, 3),
            session("done", "done", 1, 0, 4),
            session("blocked-new", "blocked", 3, 1, 5),
        ];
        sort(&mut sessions, View::Priority);
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            [
                "blocked-new",
                "blocked-old",
                "done",
                "working",
                "idle",
                "unknown",
                "saved"
            ],
        );
        // A state transition reorders the next snapshot without a view change.
        sessions
            .iter_mut()
            .find(|s| s.id == "working")
            .unwrap()
            .status = "blocked".into();
        sessions
            .iter_mut()
            .find(|s| s.id == "working")
            .unwrap()
            .state_change_seq = 101;
        sort(&mut sessions, View::Priority);
        assert_eq!(sessions[0].id, "working");
    }

    #[test]
    fn grouped_uses_native_workspace_and_pane_order_and_distinguishes_servers() {
        let mut sessions = vec![
            session("a", "blocked", 100, 1, 0),
            session("b", "working", 3, 0, 1),
            session("z", "idle", 1, 0, 0),
        ];
        sort(&mut sessions, View::Grouped);
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["z", "b", "a"],
        );
        assert_eq!(group_label(&sessions[0]), "Project 0");
        assert!(group(&sessions[0]) == group(&sessions[1]));
        assert!(group(&sessions[1]) != group(&sessions[2]));
        let mut another_server = sessions[0].clone();
        another_server.context.herdr_socket = Some("/herdr/another-socket".into());
        assert!(group(&sessions[0]) != group(&another_server));
        another_server.online = false;
        assert_eq!(group_label(&another_server), "Saved sessions");
    }

    #[test]
    fn long_names_stay_on_two_aligned_lines_in_a_fixed_height_row() {
        let ctx = egui::Context::default();
        let mut item = session("task", "idle", 1, 0, 0);
        item.display_name = Some(
            "glyph and glyphaudio\nA very long workspace name that must never expand this row"
                .into(),
        );
        item.display_agent = Some("codex".into());
        item.last_activity_at = Some(60_000);
        let mut rects = Vec::new();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.set_width(192.);
                for _ in 0..2 {
                    rects.push(
                        row(
                            ui,
                            &item,
                            super::super::session_color::PALETTE[0],
                            false,
                            180_000,
                        )
                        .rect,
                    );
                }
            });
        });
        assert_eq!(rects[0].height(), ROW_HEIGHT);
        assert_eq!(rects[1].height(), ROW_HEIGHT);
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Text(t) = &s.shape {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(texts.len(), 6);
        assert!(texts.iter().all(|t| t.galley.rows.len() == 1));
        assert_eq!(texts[0].pos.x, texts[1].pos.x);
        assert_eq!(texts[3].pos.x, texts[4].pos.x);
        assert_eq!(texts[2].pos.x, texts[5].pos.x);
    }

    #[test]
    fn recency_is_bounded_and_unknown_is_not_fabricated_as_now() {
        assert_eq!(recency(None, 900_000), "—");
        assert_eq!(recency(Some(900_001), 900_000), "now");
        assert_eq!(recency(Some(780_000), 900_000), "2 min ago");
        assert_eq!(recency(Some(1), 3_600_001), "1h ago");
        assert_eq!(recency(Some(1), 86_400_001), "1d ago");
    }
}
