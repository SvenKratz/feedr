//! One immutable view of the feed and file availability for all desktop columns.
use super::Store;
use crate::{broker::Shared, model::*};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const SCAN_INTERVAL: Duration = Duration::from_secs(2);
const DESKTOP_IDLE: Duration = Duration::from_secs(10);
const RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_COLUMNS: usize = 256;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ColumnQuery {
    pub session_id: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct BatchQuery {
    pub filters: FeedQuery,
    pub columns: Vec<ColumnQuery>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct Page {
    pub total: usize,
    pub unread: usize,
    pub posts: Vec<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Batch {
    pub snapshot: Snapshot,
    pub generation: u64,
    pub pages: Vec<Page>,
}

#[derive(Default)]
pub(crate) struct Cache {
    pub index: Option<Arc<Index>>,
    pub generation: u64,
    pub requested: Option<Instant>,
}

pub(crate) struct Index {
    catalog: Arc<Catalog>,
    availability: Vec<Available>,
}

struct Catalog {
    sequence: i64,
    posts: Vec<Row>,
    paths: Vec<String>,
}

struct Row {
    json: String,
    session: String,
    searchable: String,
    archived: bool,
    pinned: bool,
    unread: bool,
    discovery_type: Option<String>,
    types: u16,
    resources: Vec<(u16, Location)>,
}

enum Location {
    Link,
    Local(usize),
    Unavailable,
}

#[derive(Default, PartialEq, Eq)]
struct Available {
    types: u16,
    resources: u32,
}

fn type_bit(kind: &str) -> u16 {
    match kind {
        "Markdown" => 1 << 0,
        "Image" => 1 << 1,
        "PDF" => 1 << 2,
        "Video" => 1 << 3,
        "Audio" => 1 << 4,
        "Web" => 1 << 5,
        "Folder" => 1 << 6,
        "App" => 1 << 7,
        "File" => 1 << 8,
        _ => 0,
    }
}

// Read only the attachment portion of each JSON document when indexing, while
// retaining its original bytes for an internally consistent paginated reply.
#[derive(Deserialize)]
struct Attachments {
    context: AttachmentContext,
    resources: Vec<Resource>,
}

#[derive(Deserialize)]
struct AttachmentContext {
    #[serde(default = "local_host")]
    host: String,
}

impl Catalog {
    fn read(db: &mut Connection, host: &str) -> Result<Self> {
        let tx = db.transaction()?;
        let mut catalog = Self {
            sequence: Store::post_sequence(&tx)?,
            posts: Vec::new(),
            paths: Vec::new(),
        };
        let mut path_ids = HashMap::new();
        {
            let mut stmt = tx.prepare("SELECT json,session_id,searchable,archived,pinned,unread,discovery_type FROM posts ORDER BY created_at DESC,id DESC")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let json: String = row.get(0)?;
                let attachments: Attachments = serde_json::from_str(&json)?;
                let mut types = 0;
                let mut resources = Vec::with_capacity(attachments.resources.len());
                for resource in attachments.resources {
                    let bit = type_bit(resource.file_type());
                    types |= bit;
                    let location = if resource.kind == ResourceKind::Url {
                        Location::Link
                    } else if attachments.context.host == host
                        && Path::new(&resource.target).is_absolute()
                    {
                        let id = path_ids.entry(resource.target).or_insert_with_key(|path| {
                            let id = catalog.paths.len();
                            catalog.paths.push(path.clone());
                            id
                        });
                        Location::Local(*id)
                    } else {
                        Location::Unavailable
                    };
                    resources.push((bit, location));
                }
                catalog.posts.push(Row {
                    json,
                    session: row.get(1)?,
                    searchable: row.get(2)?,
                    archived: row.get(3)?,
                    pinned: row.get(4)?,
                    unread: row.get(5)?,
                    discovery_type: row.get(6)?,
                    types,
                    resources,
                });
            }
        }
        tx.commit()?;
        Ok(catalog)
    }

    fn availability(&self, paths: &[bool]) -> Vec<Available> {
        self.posts
            .iter()
            .map(|post| {
                let mut available = Available::default();
                for (index, &(kind, ref location)) in post.resources.iter().enumerate() {
                    let present = match location {
                        Location::Link => true,
                        Location::Local(path) => paths[*path],
                        Location::Unavailable => false,
                    };
                    if present && let Some(bit) = 1u32.checked_shl(index as u32) {
                        available.resources |= bit;
                        available.types |= kind;
                    }
                }
                available
            })
            .collect()
    }
}

impl Index {
    pub fn select(
        &self,
        mut snapshot: Snapshot,
        generation: u64,
        query: BatchQuery,
    ) -> Result<Batch> {
        let filter = query.filters;
        let search = filter.search.to_lowercase();
        let mut selected = filter.selected_types().peekable();
        let kinds = if selected.peek().is_none() {
            u16::MAX
        } else {
            selected.fold(0, |bits, kind| bits | type_bit(kind))
        };
        let allowed = snapshot.discovery.type_filter();
        let mut pages: Vec<Page> = (0..query.columns.len()).map(|_| Page::default()).collect();
        let mut columns = HashMap::new();
        let mut limits = Vec::new();
        let mut page_bytes = vec![0; pages.len()];
        for (index, column) in query.columns.iter().enumerate() {
            ensure!(
                columns
                    .insert(column.session_id.as_deref(), index)
                    .is_none(),
                "Duplicate feed column"
            );
            limits.push(
                FeedQuery {
                    limit: column.limit,
                    ..FeedQuery::default()
                }
                .page_limit(),
            );
        }
        let all = columns.get(&None).copied();
        let mut payload_bytes = 0;
        if !pages.is_empty() {
            for (row, available) in self.catalog.posts.iter().zip(&self.availability) {
                let session = columns.get(&Some(row.session.as_str())).copied();
                let types = if filter.available_only {
                    available.types
                } else {
                    row.types
                };
                if (all.is_none() && session.is_none())
                    || types & kinds == 0
                    || row.archived != filter.archived
                    || filter.pinned_only && !row.pinned
                    || !row.searchable.contains(&search)
                    || !filter.archived
                        && row
                            .discovery_type
                            .as_ref()
                            .is_some_and(|kind| !allowed.contains(&format!("|{kind}|")))
                {
                    continue;
                }
                let mut selected_for_page = false;
                for index in [all, session].into_iter().flatten() {
                    let page = &mut pages[index];
                    page.total += 1;
                    page.unread += usize::from(row.unread);
                    if page.posts.len() < limits[index]
                        && page_bytes[index] + row.json.len() <= RESPONSE_BYTES
                    {
                        selected_for_page = true;
                        page_bytes[index] += row.json.len();
                        page.posts.push(snapshot.posts.len());
                    } else if !filter.available_only
                        && page_bytes[index] + row.json.len() > RESPONSE_BYTES
                    {
                        // The legacy history query stops at the first oversized page row.
                        limits[index] = 0;
                    }
                }
                if selected_for_page {
                    payload_bytes += row.json.len();
                    ensure!(
                        payload_bytes <= RESPONSE_BYTES,
                        "Feed batch exceeds response limit"
                    );
                    let mut post: Post = serde_json::from_str(&row.json)?;
                    if filter.available_only {
                        let mut index = 0;
                        post.resources.retain(|_| {
                            let present = 1u32
                                .checked_shl(index)
                                .is_some_and(|bit| bit & available.resources != 0);
                            index += 1;
                            present
                        });
                    }
                    snapshot.posts.push(post);
                }
            }
        }
        Ok(Batch {
            snapshot,
            generation,
            pages,
        })
    }
}

pub(crate) fn start(shared: Shared, path: PathBuf, stopping: Arc<AtomicBool>) {
    thread::spawn(move || {
        while let Err(error) = refresh_loop(&shared, &path, &stopping) {
            eprintln!("Desktop feed index: {error:#}");
            if stopping.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn refresh_loop(shared: &Shared, path: &Path, stopping: &AtomicBool) -> Result<()> {
    let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(Duration::from_secs(2))?;
    let host = local_host();
    let mut catalog: Option<Arc<Catalog>> = None;
    let mut checks = HashMap::new();
    let mut next_scan = Instant::now();
    while !stopping.load(Ordering::Relaxed) {
        let mut state = shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("State lock failed"))?;
        if state
            .desktop
            .requested
            .is_none_or(|requested| requested.elapsed() >= DESKTOP_IDLE)
        {
            state.desktop.index = None;
            catalog = None;
            checks.clear();
            drop(shared.1.wait_timeout(state, SCAN_INTERVAL));
            continue;
        }
        let sequence = Store::post_sequence(&state.store.db)?;
        let rebuild = catalog
            .as_ref()
            .is_none_or(|catalog| catalog.sequence != sequence);
        let rescan = Instant::now() >= next_scan || catalog.is_none();
        if !rebuild && !rescan && state.desktop.index.is_some() {
            drop(
                shared
                    .1
                    .wait_timeout(state, next_scan.saturating_duration_since(Instant::now())),
            );
            continue;
        }
        drop(state);
        if rebuild {
            catalog = Some(Arc::new(Catalog::read(&mut db, &host)?));
        }
        if rescan {
            next_scan = Instant::now() + SCAN_INTERVAL;
        }
        let catalog = catalog.as_ref().expect("catalog is initialized");
        let mut previous = std::mem::take(&mut checks);
        let paths: Vec<_> = catalog
            .paths
            .iter()
            .map(|path| {
                let available = (!rescan)
                    .then(|| previous.remove(path))
                    .flatten()
                    .unwrap_or_else(|| {
                        std::fs::metadata(path).is_ok_and(|entry| entry.is_file() || entry.is_dir())
                    });
                checks.insert(path.clone(), available);
                available
            })
            .collect();
        let availability = catalog.availability(&paths);
        let mut state = shared
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("State lock failed"))?;
        // Never publish an old view after a new post or a retention deletion
        // committed during the build. Re-use the file checks on the next pass.
        if Store::post_sequence(&state.store.db)? != catalog.sequence {
            continue;
        }
        if state.desktop.index.as_ref().is_none_or(|index| {
            index.catalog.sequence != catalog.sequence || index.availability != availability
        }) {
            state.desktop.index = Some(Arc::new(Index {
                catalog: catalog.clone(),
                availability,
            }));
            state.desktop.generation += 1;
            drop(state);
            shared.1.notify_all();
        }
    }
    Ok(())
}
