use crate::model::*;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::HashMap, path::Path};

pub struct Store {
    pub(crate) db: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(
            version <= 4,
            "This database was created by a newer Feedr version"
        );
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS posts(id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), title TEXT NOT NULL, searchable TEXT NOT NULL, json TEXT NOT NULL, created_at INTEGER NOT NULL, pinned INTEGER NOT NULL, archived INTEGER NOT NULL, unread INTEGER NOT NULL, idem TEXT UNIQUE);
        CREATE INDEX IF NOT EXISTS posts_feed ON posts(archived,created_at DESC);
        CREATE INDEX IF NOT EXISTS posts_session ON posts(session_id,created_at DESC);
        CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY,value INTEGER NOT NULL);
        INSERT OR IGNORE INTO state VALUES('sequence',0);
        CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS discovery_mentions(key TEXT PRIMARY KEY,post_id TEXT NOT NULL REFERENCES posts(id),source TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS post_keys(key TEXT PRIMARY KEY,post_id TEXT NOT NULL REFERENCES posts(id));
        ")?;
        let has_types: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('posts') WHERE name='types')",
            [],
            |r| r.get(0),
        )?;
        if !has_types {
            db.execute_batch("ALTER TABLE posts ADD COLUMN types TEXT NOT NULL DEFAULT ''; ")?;
            let rows: Vec<(String, String)> = db
                .prepare("SELECT id,json FROM posts")?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?;
            for (id, json) in rows {
                let p: Post = serde_json::from_str(&json)?;
                db.execute(
                    "UPDATE posts SET types=?1 WHERE id=?2",
                    params![Self::types(&p), id],
                )?;
            }
        }
        let has_discovery_type: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('posts') WHERE name='discovery_type')",
            [],
            |row| row.get(0),
        )?;
        if !has_discovery_type {
            db.execute_batch("ALTER TABLE posts ADD COLUMN discovery_type TEXT;")?;
        }
        if version < 4 || !has_discovery_type {
            let rows: Vec<(String, String)> = db
                .prepare("SELECT id,json FROM posts")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<_, _>>()?;
            for (id, json) in rows {
                let post: Post = serde_json::from_str(&json)?;
                db.execute(
                    "UPDATE posts SET discovery_type=?1 WHERE id=?2",
                    params![Self::discovery_type(&post), id],
                )?;
            }
        }
        db.pragma_update(None, "user_version", 4)?;
        // A previous broker may have stopped without a disconnect event.
        let mut s = Self { db };
        for mut session in s.sessions()? {
            if session.online {
                session.online = false;
                s.save_session(&session)?;
            }
        }
        Ok(s)
    }
    pub fn sequence(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT value FROM state WHERE key='sequence'", [], |r| {
                r.get(0)
            })?)
    }
    pub fn touch(&self) -> Result<()> {
        Self::bump(&self.db)
    }
    pub(crate) fn bump(db: &Connection) -> Result<()> {
        db.execute("UPDATE state SET value=value+1 WHERE key='sequence'", [])?;
        Ok(())
    }
    pub fn sessions(&self) -> Result<Vec<Session>> {
        // Backfill legacy sessions from durable publications without inventing
        // an activity time for sessions first discovered during this startup.
        let mut stmt = self.db.prepare("SELECT sessions.json, MAX(posts.created_at) FROM sessions LEFT JOIN posts ON posts.session_id=sessions.id GROUP BY sessions.id ORDER BY sessions.id")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<i64>>(1)?))
        })?;
        rows.map(|r| {
            let (json, published) = r?;
            let mut session: Session = serde_json::from_str(&json)?;
            session.last_activity_at = session.last_activity_at.max(published);
            Ok(session)
        })
        .collect()
    }
    pub fn save_session(&mut self, s: &Session) -> Result<()> {
        let json = serde_json::to_string(s)?;
        let old: Option<String> = self
            .db
            .query_row("SELECT json FROM sessions WHERE id=?1", [&s.id], |r| {
                r.get(0)
            })
            .optional()?;
        if old.as_deref() == Some(&json) {
            return Ok(());
        }
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO sessions VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
            params![s.id, json],
        )?;
        Self::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
    fn bind(&mut self, mut c: Context, explicit: Option<&str>) -> Result<Session> {
        let sessions = self.sessions()?;
        if let Some(id) = explicit {
            return sessions
                .into_iter()
                .find(|s| s.id == id)
                .ok_or_else(|| anyhow::anyhow!("Unknown session ID; call feedr.sessions first"));
        }
        if let (Some(socket), Some(pane)) = (&c.herdr_socket, &c.pane_id) {
            if let Some(s) = sessions.iter().find(|s| {
                s.online
                    && s.context.host == c.host
                    && s.context.herdr_socket.as_ref() == Some(socket)
                    && s.context.pane_id.as_ref() == Some(pane)
                    && c.native_session_id
                        .as_ref()
                        .is_none_or(|id| s.context.native_session_id.as_ref() == Some(id))
            }) {
                return Ok(s.clone());
            }
            anyhow::bail!(
                "Herdr pane is not currently available; retry, pass a known session_id, or publish with an explicit unassigned context"
            );
        }
        if c.cwd.is_empty() {
            c.cwd = std::env::current_dir()?.to_string_lossy().into_owned();
        }
        // Explicit unknown pane identities never merge into arbitrary active sessions.
        let key = format!(
            "manual:{}:{}:{}:{}",
            c.host,
            c.herdr_socket.as_deref().unwrap_or(""),
            c.pane_id.as_deref().unwrap_or(""),
            c.label
        );
        if let Some(s) = sessions
            .iter()
            .find(|s| s.terminal_id == key && s.context == c)
        {
            return Ok(s.clone());
        }
        let s = Session {
            id: new_id(),
            context: c,
            status: "posted".into(),
            online: false,
            terminal_id: key,
            workspace_label: None,
            workspace_order: 0,
            pane_order: 0,
            state_change_seq: 0,
            display_name: None,
            display_agent: None,
            last_activity_at: None,
        };
        self.save_session(&s)?;
        Ok(s)
    }
    pub fn post(&mut self, input: PostInput) -> Result<Post> {
        validate_post(
            &input.title,
            &input.body,
            input.resources.len(),
            &input.readiness,
        )?;
        if let Some(k) = &input.idempotency_key {
            ensure!(!k.is_empty() && k.len() <= 512, "Invalid idempotency key");
        }
        let supplied = input.context.unwrap_or_default();
        let session = self.bind(supplied.clone(), input.session_id.as_deref())?;
        let idem = input.idempotency_key.map(|k| format!("{}:{k}", session.id));
        if let Some(key) = &idem {
            let old: Option<String> = self
                .db
                .query_row("SELECT json FROM posts WHERE idem=?1 OR id IN (SELECT post_id FROM post_keys WHERE key=?1) LIMIT 1", [key], |r| r.get(0))
                .optional()?;
            if let Some(json) = old {
                return Ok(serde_json::from_str(&json)?);
            }
        }
        // The publisher's captured cwd may differ from the pane's foreground cwd.
        let mut context = session.context.clone();
        if !supplied.cwd.is_empty() {
            context.cwd = supplied.cwd;
        }
        let resources = input
            .resources
            .into_iter()
            .map(|r| Resource::resolve(r, &context))
            .collect::<Result<Vec<_>>>()?;
        let mut p = Post {
            discovery: None,
            id: new_id(),
            session_id: session.id,
            title: input.title.trim().into(),
            body: input.body,
            resources,
            context,
            readiness: input.readiness,
            revision: 1,
            created_at: now(),
            updated_at: now(),
            pinned: false,
            archived: false,
            read: false,
        };
        if let Some(old) = self.session_posts(&p.session_id)?.into_iter().find(|old| {
            old.discovery.is_some()
                && old.context.host == p.context.host
                && old.resources.len() == p.resources.len()
                && old
                    .resources
                    .iter()
                    .zip(&p.resources)
                    .all(|(a, b)| a.target == b.target && a.kind == b.kind)
        }) {
            p.id = old.id;
            p.created_at = old.created_at;
            p.pinned = old.pinned;
            p.revision = old.revision + 1;
            p.discovery = old.discovery.map(|mut origin| {
                origin.accepted = true;
                origin
            });
        }
        let tx = self.db.transaction()?;
        Self::write_post(&tx, &p, idem.as_deref())?;
        Self::write_activity(&tx, &p.session_id, p.updated_at)?;
        if let Some(key) = &idem {
            tx.execute(
                "INSERT OR IGNORE INTO post_keys VALUES(?1,?2)",
                params![key, p.id],
            )?;
        }
        Self::bump(&tx)?;
        tx.commit()?;
        Ok(p)
    }
    pub(crate) fn write_post(db: &Connection, p: &Post, idem: Option<&str>) -> Result<()> {
        let searchable = format!(
            "{} {} {}",
            p.title,
            p.body,
            p.resources
                .iter()
                .map(|r| r.target.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        )
        .to_lowercase();
        db.execute("INSERT INTO posts(id,session_id,title,searchable,json,created_at,pinned,archived,unread,idem,types,discovery_type) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(id) DO UPDATE SET title=excluded.title,searchable=excluded.searchable,json=excluded.json,pinned=excluded.pinned,archived=excluded.archived,unread=excluded.unread,types=excluded.types,discovery_type=excluded.discovery_type",params![p.id,p.session_id,p.title,searchable,serde_json::to_string(p)?,p.created_at,p.pinned,p.archived,!p.read,idem,Self::types(p),Self::discovery_type(p)])?;
        Ok(())
    }
    pub fn get(&self, id: &str) -> Result<Post> {
        let json: String = self
            .db
            .query_row("SELECT json FROM posts WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("Post not found"))?;
        Ok(serde_json::from_str(&json)?)
    }
    pub fn update(&mut self, u: UpdateInput) -> Result<Post> {
        let publication =
            u.title.is_some() || u.body.is_some() || u.resources.is_some() || u.readiness.is_some();
        let mut p = self.get(&u.id)?;
        ensure!(
            p.revision == u.expected_revision,
            "Revision conflict: current revision is {}",
            p.revision
        );
        if let Some(t) = u.title {
            p.title = t.trim().into();
        }
        if let Some(b) = u.body {
            p.body = b;
        }
        if let Some(r) = u.resources {
            p.resources = r
                .into_iter()
                .map(|r| Resource::resolve(r, &p.context))
                .collect::<Result<_>>()?;
        }
        if let Some(r) = u.readiness {
            p.readiness = r;
        }
        if let Some(v) = u.pinned {
            p.pinned = v;
        }
        if let Some(v) = u.archived {
            p.archived = v;
        }
        if let Some(v) = u.read {
            p.read = v;
        }
        validate_post(&p.title, &p.body, p.resources.len(), &p.readiness)?;
        p.revision += 1;
        p.updated_at = now();
        let tx = self.db.transaction()?;
        Self::write_post(&tx, &p, None)?;
        if publication {
            Self::write_activity(&tx, &p.session_id, p.updated_at)?;
        }
        Self::bump(&tx)?;
        tx.commit()?;
        Ok(p)
    }
    fn write_activity(db: &Connection, id: &str, timestamp: i64) -> Result<()> {
        let json: String =
            db.query_row("SELECT json FROM sessions WHERE id=?1", [id], |r| r.get(0))?;
        let mut session: Session = serde_json::from_str(&json)?;
        session.last_activity_at = session.last_activity_at.max(Some(timestamp));
        db.execute(
            "UPDATE sessions SET json=?1 WHERE id=?2",
            params![serde_json::to_string(&session)?, id],
        )?;
        Ok(())
    }
    fn types(p: &Post) -> String {
        format!(
            "|{}|",
            p.resources
                .iter()
                .map(Resource::file_type)
                .collect::<Vec<_>>()
                .join("|")
        )
    }
    fn discovery_type(post: &Post) -> Option<&'static str> {
        if post.pinned || post.discovery.as_ref().is_none_or(|origin| origin.accepted) {
            return None;
        }
        post.resources
            .first()
            .map(|resource| crate::discovery::FileType::of(resource).key())
    }
    pub fn snapshot(&self, q: &FeedQuery) -> Result<Snapshot> {
        let limit = if q.limit == 0 {
            200
        } else {
            q.limit.clamp(1, 2000)
        };
        let discovery = self.discovery_config()?;
        let allowed = discovery.type_filter();
        let filter = "archived=?1 AND (?2 IS NULL OR session_id=?2) AND (?3=0 OR pinned=1) AND instr(searchable,?4)>0 AND (?5 IS NULL OR instr(types,'|'||?5||'|')>0) AND (?1=1 OR discovery_type IS NULL OR instr(?6,'|'||discovery_type||'|')>0)";
        let search = q.search.to_lowercase();
        let (mut total, mut unread): (usize, usize) = if q.available_only {
            (0, 0)
        } else {
            self.db.query_row(
                &format!("SELECT count(*),coalesce(sum(unread),0) FROM posts WHERE {filter}"),
                params![
                    q.archived,
                    q.session_id,
                    q.pinned_only,
                    search,
                    q.kind,
                    allowed
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?
        };
        let mut stmt = self.db.prepare(&format!(
            "SELECT json FROM posts WHERE {filter} ORDER BY created_at DESC,id DESC LIMIT ?7"
        ))?;
        let rows = stmt.query_map(
            params![
                q.archived,
                q.session_id,
                q.pinned_only,
                search,
                q.kind,
                allowed,
                if q.available_only {
                    -1_i64
                } else {
                    limit as i64
                }
            ],
            |r| r.get::<_, String>(0),
        )?;
        let mut posts = Vec::new();
        let mut bytes = 0;
        let mut availability = HashMap::new();
        for row in rows {
            let json = row?;
            let mut post: Post = serde_json::from_str(&json)?;
            if q.available_only {
                let context = &post.context;
                post.resources.retain(|resource| {
                    *availability
                        .entry((context.host.clone(), resource.target.clone()))
                        .or_insert_with(|| {
                            if resource.kind == ResourceKind::Url {
                                return true;
                            }
                            if !context.is_local() || !Path::new(&resource.target).is_absolute() {
                                return false;
                            }
                            std::fs::metadata(&resource.target)
                                .is_ok_and(|metadata| metadata.is_file() || metadata.is_dir())
                        })
                });
                if post.resources.is_empty()
                    || q.kind
                        .as_ref()
                        .is_some_and(|kind| !post.resources.iter().any(|r| r.file_type() == kind))
                {
                    continue;
                }
                total += 1;
                unread += usize::from(!post.read);
            }
            // Inspect every matching post for accurate counts, while bounding
            // the returned page and preserving the complete durable post JSON.
            if posts.len() >= limit {
                continue;
            }
            if bytes + json.len() > 32 * 1024 * 1024 {
                if q.available_only { continue } else { break }
            }
            bytes += json.len();
            posts.push(post);
        }
        Ok(Snapshot {
            discovery,
            discovery_error: None,
            sequence: self.sequence()?,
            posts,
            sessions: self.sessions()?,
            total,
            unread,
            connections: vec![],
        })
    }
    pub fn reconcile(&mut self, socket: &str, incoming: Vec<Session>) -> Result<()> {
        let mut existing: HashMap<_, _> = self
            .sessions()?
            .into_iter()
            .map(|s| (s.id.clone(), s))
            .collect();
        for s in &incoming {
            existing.remove(&s.id);
            self.save_session(s)?;
        }
        for (_, mut s) in existing {
            if s.context.herdr_socket.as_deref() == Some(socket) && s.online {
                s.online = false;
                s.status = "offline".into();
                self.save_session(&s)?;
            }
        }
        Ok(())
    }
}
