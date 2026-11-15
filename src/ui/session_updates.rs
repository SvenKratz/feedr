//! A visit acknowledges a fixed update window, never a background refresh.
use crate::model::{Post, Session};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

type Item = (String, String);

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Cursor {
    through: Option<i64>,
    posts: HashMap<String, SeenPost>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SeenPost {
    created_at: i64,
    resources: HashSet<String>,
}

#[derive(PartialEq, Eq)]
struct Checkpoint {
    status: String,
    sequence: u64,
    activity: Option<i64>,
}

struct Batch {
    checkpoint: Option<Checkpoint>,
    through: i64,
    previous: Cursor,
    highlighted: HashSet<Item>,
}

#[derive(Default)]
pub(super) struct Updates {
    pub cursors: HashMap<String, Cursor>,
    batches: HashMap<String, Batch>,
}

impl Updates {
    pub fn restore(cursors: HashMap<String, Cursor>) -> Self {
        Self {
            cursors,
            ..Self::default()
        }
    }
    pub fn open(&mut self, id: &str, session: Option<&Session>, posts: &[Post], through: i64) {
        let checkpoint = session.map(|s| Checkpoint {
            status: s.status.clone(),
            sequence: s.state_change_seq,
            activity: s.last_activity_at,
        });
        if let Some(batch) = self.batches.get_mut(id)
            && batch.checkpoint == checkpoint
            && !posts.iter().any(|post| {
                post.session_id == id
                    && post.updated_at > batch.through
                    && post.resources.iter().any(|resource| {
                        !self.cursors.get(id).is_some_and(|cursor| {
                            cursor
                                .posts
                                .get(&post.id)
                                .is_some_and(|seen| seen.resources.contains(&resource.target))
                        })
                    })
            })
        {
            // Revisit the same checkmark/update without erasing its highlights.
            batch.through = through;
            self.receive(id, posts);
            return;
        }
        self.batches.insert(
            id.into(),
            Batch {
                checkpoint,
                through,
                previous: self.cursors.get(id).cloned().unwrap_or_default(),
                highlighted: HashSet::new(),
            },
        );
        self.receive(id, posts);
    }

    pub fn receive(&mut self, id: &str, posts: &[Post]) {
        if posts.is_empty() {
            return;
        }
        let Some(batch) = self.batches.get_mut(id) else {
            return;
        };
        let cursor = self.cursors.entry(id.into()).or_default();
        for post in posts.iter().filter(|p| {
            p.session_id == id && p.created_at <= batch.through && p.updated_at <= batch.through
        }) {
            let previous = batch.previous.posts.get(&post.id);
            let new_post = match batch.previous.through {
                Some(at) => post.created_at >= at,
                None => !post.read,
            };
            let seen = cursor
                .posts
                .entry(post.id.clone())
                .or_insert_with(|| SeenPost {
                    created_at: post.created_at,
                    resources: HashSet::new(),
                });
            for resource in &post.resources {
                if (new_post || previous.is_some())
                    && !previous.is_some_and(|p| p.resources.contains(&resource.target))
                {
                    batch
                        .highlighted
                        .insert((post.id.clone(), resource.target.clone()));
                }
                seen.resources.insert(resource.target.clone());
            }
        }
        cursor.through = Some(cursor.through.unwrap_or_default().max(batch.through));
        // Bound saved UI state even for sessions with years of deliverables.
        if cursor.posts.len() > 2000 {
            let mut ordered: Vec<_> = cursor
                .posts
                .iter()
                .map(|(id, p)| (id.clone(), p.created_at))
                .collect();
            ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let keep: HashSet<_> = ordered.into_iter().take(2000).map(|(id, _)| id).collect();
            cursor.posts.retain(|id, _| keep.contains(id));
        }
    }

    pub fn highlighted(&self, id: &str, post: &Post, resource: &str) -> bool {
        self.batches.get(id).is_some_and(|batch| {
            batch
                .highlighted
                .contains(&(post.id.clone(), resource.into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Context, Resource, ResourceKind};

    fn post(id: &str, at: i64) -> Post {
        Post {
            id: id.into(),
            session_id: "session".into(),
            title: id.into(),
            body: String::new(),
            resources: vec![Resource {
                original: id.into(),
                target: id.into(),
                name: id.into(),
                kind: ResourceKind::File,
            }],
            context: Context::default(),
            readiness: "ready".into(),
            revision: 1,
            created_at: at,
            updated_at: at,
            pinned: false,
            archived: false,
            read: false,
            discovery: None,
        }
    }

    #[test]
    fn only_additions_since_the_previous_visit_are_highlighted() {
        let mut updates = Updates::default();
        let old = post("old", 1);
        updates.open("session", None, std::slice::from_ref(&old), 10);
        let first = post("first", 11);
        let second = post("second", 12);
        // Polls before the click cannot acknowledge the next batch.
        updates.receive("session", &[second.clone(), first.clone(), old.clone()]);
        assert!(!updates.highlighted("session", &first, "first"));
        updates.open(
            "session",
            None,
            &[second.clone(), first.clone(), old.clone()],
            20,
        );
        assert!(updates.highlighted("session", &first, "first"));
        assert!(updates.highlighted("session", &second, "second"));
        assert!(!updates.highlighted("session", &old, "old"));
        updates.open("session", None, &[second, first.clone(), old], 21);
        assert!(updates.highlighted("session", &first, "first"));
    }

    #[test]
    fn fresh_done_checkpoint_replaces_the_previous_batch_and_stays_session_local() {
        let mut updates = Updates::default();
        let old = post("old", 1);
        let mut session:Session=serde_json::from_value(serde_json::json!({"id":"session","context":Context::default(),"status":"done","online":true,"terminal_id":"test","state_change_seq":1})).unwrap();
        updates.open("session", Some(&session), std::slice::from_ref(&old), 10);
        session.state_change_seq = 3;
        let mut new = post("new", 15);
        // Reading it in All sessions does not acknowledge this session column.
        new.read = true;
        updates.open("session", Some(&session), &[new.clone(), old.clone()], 20);
        assert!(updates.highlighted("session", &new, "new"));
        assert!(!updates.highlighted("session", &old, "old"));
        assert!(!updates.highlighted("other", &new, "new"));
        assert!(!updates.highlighted("session", &new, "other-attachment"));
    }

    #[test]
    fn delayed_snapshot_and_metadata_updates_do_not_erase_a_visit_batch() {
        let mut updates = Updates::default();
        let old = post("old", 1);
        updates.open("session", None, std::slice::from_ref(&old), 10);
        updates.open("session", None, std::slice::from_ref(&old), 20);
        // Opening an uncached column may initially have only the old snapshot.
        let mut new = post("new", 15);
        updates.receive("session", &[new.clone(), old]);
        assert!(updates.highlighted("session", &new, "new"));
        new.updated_at = 25;
        new.read = true;
        new.pinned = true;
        new.revision += 1;
        updates.receive("session", std::slice::from_ref(&new));
        assert!(updates.highlighted("session", &new, "new"));
        let future = post("future", 30);
        updates.receive("session", std::slice::from_ref(&future));
        assert!(!updates.highlighted("session", &future, "future"));
        updates.open("session", None, &[future.clone(), new], 40);
        assert!(updates.highlighted("session", &future, "future"));
    }

    #[test]
    fn saved_cursor_distinguishes_new_attachments_from_old_history_and_read_flags() {
        let mut updates = Updates::default();
        let mut old = post("old", 1);
        old.read = true;
        updates.open("session", None, std::slice::from_ref(&old), 10);
        assert!(!updates.highlighted("session", &old, "old"));
        let saved = serde_json::to_string(&updates.cursors).unwrap();
        let mut restored = Updates {
            cursors: serde_json::from_str(&saved).unwrap(),
            ..Updates::default()
        };
        old.updated_at = 15;
        old.resources.push(Resource {
            original: "added".into(),
            target: "added".into(),
            name: "added".into(),
            kind: ResourceKind::File,
        });
        let mut history = post("history", 0);
        history.read = false;
        restored.open("session", None, &[old.clone(), history.clone()], 20);
        assert!(restored.highlighted("session", &old, "added"));
        assert!(!restored.highlighted("session", &old, "old"));
        assert!(!restored.highlighted("session", &history, "history"));
    }
}
