use super::{Config, Ingested, Input, Origin, extract};
use crate::{model::*, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

impl Store {
    pub(crate) fn setting<T: DeserializeOwned + Default>(&self, key: &str) -> Result<T> {
        let json: Option<String> = self
            .db
            .query_row("SELECT json FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .unwrap_or_else(|| Ok(T::default()))
    }
    pub(crate) fn set_setting(&mut self, key: &str, value: &impl Serialize) -> Result<()> {
        let json = serde_json::to_string(value)?;
        let changed = self.db.execute("INSERT INTO settings VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json WHERE settings.json!=excluded.json", params![key,json])?;
        if changed > 0 {
            Self::bump(&self.db)?;
        }
        Ok(())
    }
    pub fn discovery_config(&self) -> Result<Config> {
        self.setting("discovery.config")
    }
    pub fn set_discovery_config(&mut self, config: &Config) -> Result<()> {
        config.validate()?;
        let sessions = self.sessions()?;
        let old = self.discovery_config()?;
        for log in &config.logs {
            ensure!(
                sessions.iter().any(|session| session.id == log.session_id),
                "Unknown log session; use feedr sessions to select one"
            );
            if !old.logs.contains(log) {
                self.set_setting(
                    &super::watch::cursor_key(log),
                    &super::watch::initial_cursor(log),
                )?;
            }
        }
        self.set_setting("discovery.config", config)
    }
    pub(crate) fn session_posts(&self, session: &str) -> Result<Vec<Post>> {
        let mut statement = self
            .db
            .prepare("SELECT json FROM posts WHERE session_id=?1")?;
        let rows = statement.query_map([session], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }
    pub fn ingest(&mut self, input: &Input) -> Result<Ingested> {
        ensure!(
            input.text.len() <= 128 * 1024 && input.source.len() <= 4096,
            "Mention input exceeds size limit"
        );
        let config = self.discovery_config()?;
        ensure!(
            config.includes(&input.session_id),
            "Discovery is disabled for this session"
        );
        let session = self
            .sessions()?
            .into_iter()
            .find(|session| session.id == input.session_id)
            .context("Unknown discovery session; use feedr sessions to select one")?;
        let mut posts = self.session_posts(&session.id)?;
        let mut result = Ingested::default();
        for locator in extract(&input.text, &config) {
            let resolved = Resource::resolve(
                ResourceInput {
                    uri: locator.clone(),
                    kind: None,
                },
                &session.context,
            );
            let unresolved = resolved.is_err();
            let resource = resolved.unwrap_or_else(|_| Resource {
                original: locator.clone(),
                target: locator.clone(),
                name: locator.rsplit('/').next().unwrap_or(&locator).to_string(),
                kind: ResourceKind::File,
            });
            if !config.allows(&resource) {
                continue;
            }
            if result.created.len() + result.matched.len() >= 32 {
                break;
            }
            let key = format!(
                "{:x}",
                Sha256::digest(format!(
                    "{}\0{}\0{}",
                    session.id, session.context.host, resource.target
                ))
            );
            let existing: Option<String> = self
                .db
                .query_row(
                    "SELECT post_id FROM discovery_mentions WHERE key=?1",
                    [&key],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = existing {
                result.matched.push(id);
                continue;
            }
            if let Some(post) = posts.iter().find(|post| {
                post.context.host == session.context.host
                    && post.resources.iter().any(|r| r.target == resource.target)
            }) {
                self.db.execute(
                    "INSERT OR IGNORE INTO discovery_mentions VALUES(?1,?2,?3)",
                    params![key, post.id, input.source],
                )?;
                result.matched.push(post.id.clone());
                continue;
            }
            let ready = resource.kind == ResourceKind::Url;
            let post = Post {
                id: new_id(),
                session_id: session.id.clone(),
                title: resource.name.clone(),
                body: String::new(),
                resources: vec![resource],
                context: session.context.clone(),
                readiness: if unresolved {
                    "unresolved"
                } else if ready {
                    "ready"
                } else {
                    "creating"
                }
                .into(),
                revision: 1,
                created_at: now(),
                updated_at: now(),
                pinned: false,
                archived: false,
                read: false,
                discovery: Some(Origin {
                    source: input.source.clone(),
                    excerpt: input.text.chars().take(400).collect(),
                    locator,
                    accepted: false,
                    unresolved,
                }),
            };
            let tx = self.db.transaction()?;
            Self::write_post(&tx, &post, Some(&format!("discovered:{key}")))?;
            tx.execute(
                "INSERT INTO discovery_mentions VALUES(?1,?2,?3)",
                params![key, post.id, input.source],
            )?;
            Self::bump(&tx)?;
            tx.commit()?;
            result.created.push(post.id.clone());
            posts.push(post);
        }
        result.matched.sort();
        result.matched.dedup();
        Ok(result)
    }
    pub fn keep_discovery(&mut self, id: &str, revision: i64) -> Result<Post> {
        let mut post = self.get(id)?;
        ensure!(
            post.revision == revision,
            "Revision conflict; refresh this mention"
        );
        post.discovery
            .as_mut()
            .context("This post is not a discovered mention")?
            .accepted = true;
        post.revision += 1;
        post.updated_at = now();
        let tx = self.db.transaction()?;
        Self::write_post(&tx, &post, None)?;
        Self::bump(&tx)?;
        tx.commit()?;
        Ok(post)
    }
    pub(crate) fn pending_mentions(&self) -> Result<Vec<Post>> {
        let mut statement = self.db.prepare(
            "SELECT json FROM posts WHERE archived=0 AND json_extract(json,'$.discovery') IS NOT NULL AND json_extract(json,'$.readiness')!='ready' ORDER BY created_at DESC LIMIT 256",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut posts = Vec::new();
        for row in rows {
            let post: Post = serde_json::from_str(&row?)?;
            if post.discovery.is_some() && post.readiness != "ready" {
                posts.push(post);
            }
        }
        Ok(posts)
    }
    pub(crate) fn settle_mention(&mut self, id: &str) -> Result<()> {
        let mut post = self.get(id)?;
        if post.readiness == "ready" || post.archived {
            return Ok(());
        }
        if let Some(origin) = &post.discovery
            && let Ok(resource) = Resource::resolve(
                ResourceInput {
                    uri: origin.locator.clone(),
                    kind: None,
                },
                &post.context,
            )
        {
            post.resources = vec![resource];
        }
        post.readiness = "ready".into();
        post.revision += 1;
        post.updated_at = now();
        let tx = self.db.transaction()?;
        Self::write_post(&tx, &post, None)?;
        Self::bump(&tx)?;
        tx.commit()?;
        Ok(())
    }
}
