//! Saved column identity/order is separate from each column's live query window.
use crate::model::{FeedQuery, Session, Snapshot};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) enum Target {
    All,
    Session(String),
}
impl Target {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Session(id) => Some(id),
        }
    }
    pub fn title(&self, sessions: &[Session]) -> String {
        self.compact_title(sessions, usize::MAX)
    }
    pub fn compact_title(&self, sessions: &[Session], max_chars: usize) -> String {
        match self {
            Self::All => "All sessions".into(),
            Self::Session(id) => sessions
                .iter()
                .find(|s| &s.id == id)
                .map(|s| {
                    let agent = match super::session_list::agent(s).to_lowercase().as_str() {
                        "claude" => "Claude",
                        "codex" => "Codex",
                        _ => super::session_list::agent(s),
                    };
                    let label = super::ellipsize(
                        super::session_list::name(s),
                        max_chars.saturating_sub(agent.chars().count() + 3).max(3),
                    );
                    format!("{label} ({agent})")
                })
                .unwrap_or_else(|| "Saved session".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Layout {
    pub order: Vec<Target>,
    pub active: Option<Target>,
    #[serde(default)]
    pub newest_first: bool,
    #[serde(skip)]
    sidebar_focus: Option<Target>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            order: vec![Target::All],
            active: Some(Target::All),
            newest_first: false,
            sidebar_focus: None,
        }
    }
}
impl Layout {
    pub fn display_order(&self, sessions: &[Session]) -> Vec<Target> {
        let mut order = self.order.clone();
        if self.newest_first {
            let activity: std::collections::HashMap<_, _> = sessions
                .iter()
                .map(|session| (session.id.as_str(), session.last_activity_at))
                .collect();
            // Stable ties retain the user's manual order. The catchall goes last.
            order.sort_by(|a, b| match (a, b) {
                (Target::All, Target::All) => std::cmp::Ordering::Equal,
                (Target::All, _) => std::cmp::Ordering::Greater,
                (_, Target::All) => std::cmp::Ordering::Less,
                (Target::Session(a), Target::Session(b)) => activity
                    .get(b.as_str())
                    .copied()
                    .flatten()
                    .cmp(&activity.get(a.as_str()).copied().flatten()),
            });
            if let Some(focused) = &self.sidebar_focus
                && let Some(index) = order.iter().position(|target| target == focused)
            {
                order[..=index].rotate_right(1);
            }
        }
        order
    }

    pub fn toggle_newest_first(&mut self) {
        self.newest_first = !self.newest_first;
        self.sidebar_focus = None;
    }

    pub fn drag_to(
        &mut self,
        displayed: &[Target],
        source: &Target,
        destination: &Target,
        after: bool,
    ) {
        if source == destination
            || !self.order.contains(source)
            || !self.order.contains(destination)
        {
            return;
        }
        if self.newest_first {
            // A direct drag takes over from automatic ordering using the exact
            // arrangement the user grabbed, even if activity changed mid-drag.
            self.order = displayed.to_vec();
            self.normalize();
            self.toggle_newest_first();
        }
        self.move_to(source, destination, after);
    }

    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.order.retain(|target| seen.insert(target.clone()));
        if !self
            .active
            .as_ref()
            .is_some_and(|target| self.order.contains(target))
        {
            self.active = self.order.first().cloned();
        }
    }
    pub fn open(&mut self, target: Target) {
        if !self.order.contains(&target) {
            self.order.push(target.clone());
        }
        self.active = Some(target);
    }
    pub fn open_first(&mut self, target: Target) {
        self.open(target.clone());
        if self.newest_first {
            self.sidebar_focus = Some(target);
        } else if let Some(index) = self.order.iter().position(|open| *open == target) {
            self.order[..=index].rotate_right(1);
        }
    }
    pub fn close(&mut self, target: &Target) {
        let Some(index) = self.order.iter().position(|t| t == target) else {
            return;
        };
        self.order.remove(index);
        if self.sidebar_focus.as_ref() == Some(target) {
            self.sidebar_focus = None;
        }
        if self.active.as_ref() == Some(target) {
            self.active = self
                .order
                .get(index.min(self.order.len().saturating_sub(1)))
                .cloned();
        }
    }
    pub fn move_to(&mut self, source: &Target, destination: &Target, after: bool) {
        if source == destination || !self.order.contains(destination) {
            return;
        }
        let Some(from) = self.order.iter().position(|t| t == source) else {
            return;
        };
        let target = self.order.remove(from);
        let to = self.order.iter().position(|t| t == destination).unwrap();
        self.order.insert(to + usize::from(after), target);
    }
}

#[derive(Default)]
pub(super) struct Feed {
    pub snapshot: Snapshot,
    pub limit: usize,
    pub reveal_latest: bool,
}
impl Feed {
    pub fn query(&self, filters: &FeedQuery, target: &Target) -> FeedQuery {
        FeedQuery {
            session_id: target.session_id().map(str::to_owned),
            limit: self.limit,
            ..filters.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, activity: Option<i64>) -> Session {
        serde_json::from_value(serde_json::json!({
            "id":id,"context":{},"status":"idle","online":true,
            "terminal_id":id,"last_activity_at":activity
        }))
        .unwrap()
    }

    #[test]
    fn activity_order_is_stable_and_reversible_with_the_catchall_last() {
        let manual = vec![
            Target::All,
            Target::Session("a".into()),
            Target::Session("unknown".into()),
            Target::Session("b".into()),
            Target::Session("tie".into()),
            Target::Session("missing".into()),
        ];
        let mut layout = Layout {
            order: manual.clone(),
            active: Some(Target::Session("a".into())),
            newest_first: true,
            ..Layout::default()
        };
        let sessions = vec![
            session("a", Some(100)),
            session("b", Some(200)),
            session("tie", Some(100)),
            session("unknown", None),
        ];
        assert_eq!(
            layout.display_order(&sessions),
            vec![
                Target::Session("b".into()),
                Target::Session("a".into()),
                Target::Session("tie".into()),
                Target::Session("unknown".into()),
                Target::Session("missing".into()),
                Target::All
            ]
        );
        assert_eq!(layout.order, manual);
        assert_eq!(layout.active, Some(Target::Session("a".into())));
        layout.newest_first = false;
        assert_eq!(layout.display_order(&sessions), manual);
    }

    #[test]
    fn automatic_order_persists_and_legacy_layouts_keep_manual_order() {
        let mut layout: Layout = serde_json::from_value(serde_json::json!({
            "order":["All",{"Session":"a"}],"active":{"Session":"a"}
        }))
        .unwrap();
        assert!(!layout.newest_first);
        layout.newest_first = true;
        let restored: Layout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        assert!(restored.newest_first);
        assert_eq!(restored.order, layout.order);
        assert_eq!(restored.active, layout.active);
    }

    #[test]
    fn dragging_uses_the_visible_order_and_switches_to_manual() {
        let a = Target::Session("a".into());
        let b = Target::Session("b".into());
        let mut layout = Layout {
            order: vec![Target::All, a.clone(), b.clone()],
            active: Some(a.clone()),
            newest_first: true,
            ..Layout::default()
        };
        let visible = vec![b.clone(), a.clone(), Target::All];
        layout.drag_to(&visible, &b, &Target::All, true);
        assert!(!layout.newest_first);
        assert_eq!(layout.order, vec![a.clone(), Target::All, b]);
        assert_eq!(layout.active, Some(a));
    }
    #[test]
    fn closed_catchall_stays_closed_and_saved_order_survives() {
        let mut layout = Layout::default();
        let a = Target::Session("a".into());
        let b = Target::Session("b".into());
        layout.open(a.clone());
        layout.open(b.clone());
        layout.open(a.clone());
        assert_eq!(layout.order.len(), 3);
        layout.move_to(&b, &Target::All, false);
        layout.close(&Target::All);
        let mut restored: Layout =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        restored.normalize();
        assert_eq!(restored.order, [b, a.clone()]);
        assert_eq!(restored.active, Some(a.clone()));
        restored.close(&a);
        restored.close(&Target::Session("b".into()));
        restored.normalize();
        assert!(restored.order.is_empty());
        assert!(restored.active.is_none());
        restored.open(Target::All);
        assert_eq!(restored.order, [Target::All]);
    }
    #[test]
    fn reordering_both_directions_preserves_active_column() {
        let mut layout = Layout::default();
        let a = Target::Session("a".into());
        let b = Target::Session("b".into());
        layout.open(a.clone());
        layout.open(b.clone());
        layout.move_to(&Target::All, &b, true);
        assert_eq!(layout.order, [a.clone(), b.clone(), Target::All]);
        layout.move_to(&Target::All, &a, false);
        assert_eq!(layout.order, [Target::All, a, b.clone()]);
        assert_eq!(layout.active, Some(b));
    }
    #[test]
    fn each_column_keeps_its_session_and_pagination_with_shared_filters() {
        let filters = FeedQuery {
            search: "report".into(),
            pinned_only: true,
            ..FeedQuery::default()
        };
        let feed = Feed {
            limit: 600,
            ..Feed::default()
        };
        let query = feed.query(&filters, &Target::Session("quiet-session".into()));
        assert_eq!(query.session_id.as_deref(), Some("quiet-session"));
        assert_eq!(query.search, "report");
        assert!(query.pinned_only);
        assert_eq!(query.limit, 600);
        assert_eq!(Feed::default().query(&filters, &Target::All).limit, 0);
        assert!(filters.session_id.is_none());
    }
}
