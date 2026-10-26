//! Match herdr's native workspace order and status/transition priority.
use crate::model::Session;
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
}
