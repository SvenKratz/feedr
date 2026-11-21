use super::Store;
use crate::model::Retention;
use anyhow::Result;
use rusqlite::{Connection, params};

const KEY: &str = "feed.retention";

impl Store {
    pub fn retention(&self) -> Result<Retention> {
        self.setting(KEY)
    }

    pub fn set_retention(&mut self, policy: Retention, now: i64) -> Result<usize> {
        let tx = self.db.transaction()?;
        let changed = tx.execute(
            "INSERT INTO settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json WHERE settings.json!=excluded.json",
            params![KEY, serde_json::to_string(&policy)?],
        )?;
        let deleted = delete_expired(&tx, policy, now)?;
        if changed > 0 || deleted > 0 {
            Self::bump(&tx)?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    pub fn prune_expired(&mut self, now: i64) -> Result<usize> {
        let policy = self.retention()?;
        if policy == Retention::Forever {
            return Ok(0);
        }
        let tx = self.db.transaction()?;
        let deleted = delete_expired(&tx, policy, now)?;
        if deleted > 0 {
            Self::bump(&tx)?;
        }
        tx.commit()?;
        Ok(deleted)
    }
}

fn delete_expired(db: &Connection, policy: Retention, now: i64) -> Result<usize> {
    let shift = match policy {
        Retention::Forever => return Ok(0),
        Retention::Year => "-1 year",
        Retention::SixMonths => "-6 months",
        Retention::Days90 => "-90 days",
        Retention::Days30 => "-30 days",
        Retention::Days14 => "-14 days",
    };
    // Calendar windows use UTC, with month-end dates clamped to the last day.
    let cutoff: i64 = db.query_row(
        "SELECT unixepoch(?1 / 1000,'unixepoch',?2,'floor') * 1000 + ?1 % 1000",
        params![now, shift],
        |row| row.get(0),
    )?;
    for table in ["discovery_mentions", "post_keys"] {
        db.execute(
            &format!("DELETE FROM {table} WHERE post_id IN (SELECT id FROM posts WHERE pinned=0 AND created_at < ?1)"),
            [cutoff],
        )?;
    }
    let deleted = db.execute(
        "DELETE FROM posts WHERE pinned=0 AND created_at < ?1",
        [cutoff],
    )?;
    if deleted > 0 {
        Store::bump_posts(db)?;
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PostInput;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn finite_periods_keep_posts_exactly_at_the_utc_cutoff() {
        for (policy, date) in [
            (Retention::Year, "2023-08-31"),
            (Retention::SixMonths, "2024-02-29"),
            (Retention::Days90, "2024-06-02"),
            (Retention::Days30, "2024-08-01"),
            (Retention::Days14, "2024-08-17"),
        ] {
            let mut store = Store::open(Path::new(":memory:")).unwrap();
            let utc = |date| -> i64 {
                store
                    .db
                    .query_row("SELECT unixepoch(?1) * 1000 + 123", [date], |row| {
                        row.get(0)
                    })
                    .unwrap()
            };
            let now = utc("2024-08-31");
            let cutoff = utc(date);
            let mut ids = Vec::new();
            for created_at in [cutoff - 1, cutoff] {
                let input: PostInput = serde_json::from_value(json!({
                    "title":"Fixture", "resources":[{"uri":"https://example.invalid/fixture"}]
                }))
                .unwrap();
                let post = store.post(input).unwrap();
                store
                    .db
                    .execute(
                        "UPDATE posts SET created_at=?1 WHERE id=?2",
                        params![created_at, post.id],
                    )
                    .unwrap();
                ids.push(post.id);
            }
            assert_eq!(store.set_retention(policy, now).unwrap(), 1, "{policy:?}");
            assert!(store.get(&ids[0]).is_err(), "{policy:?}");
            assert!(store.get(&ids[1]).is_ok(), "{policy:?}");
            assert_eq!(store.prune_expired(now).unwrap(), 0);
            assert_eq!(store.prune_expired(now + 1).unwrap(), 1);
        }
    }
}
