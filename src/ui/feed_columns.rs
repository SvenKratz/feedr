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
                    let agent = match s.context.agent.to_lowercase().as_str() {
                        "claude" => "Claude",
                        "codex" => "Codex",
                        _ => &s.context.agent,
                    };
                    let label = super::ellipsize(
                        &s.context.label,
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
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            order: vec![Target::All],
            active: Some(Target::All),
        }
    }
}
impl Layout {
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
    pub fn close(&mut self, target: &Target) {
        let Some(index) = self.order.iter().position(|t| t == target) else {
            return;
        };
        self.order.remove(index);
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
    pub pending: Option<Snapshot>,
    pub limit: usize,
}
impl Feed {
    pub fn query(&self, filters: &FeedQuery, target: &Target) -> FeedQuery {
        FeedQuery {
            session_id: target.session_id().map(str::to_owned),
            limit: self.limit,
            ..filters.clone()
        }
    }
    pub fn receive(&mut self, snapshot: Snapshot, reading: bool) {
        let additions = snapshot
            .posts
            .iter()
            .any(|p| !self.snapshot.posts.iter().any(|old| old.id == p.id));
        if reading && additions && !self.snapshot.posts.is_empty() {
            self.snapshot.posts.retain_mut(|post| {
                if let Some(new) = snapshot.posts.iter().find(|p| p.id == post.id) {
                    *post = new.clone();
                    true
                } else {
                    false
                }
            });
            self.snapshot.total = snapshot.total;
            self.snapshot.unread = snapshot.unread;
            self.pending = Some(snapshot);
        } else {
            self.snapshot = snapshot;
            self.pending = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
