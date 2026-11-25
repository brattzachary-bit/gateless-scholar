//! SQLite storage. One connection behind a mutex; every call is short, so background tasks
//! may call it directly.
//!
//! What is permanent: favourites, read later, bookshelves, projects, notes and the articles
//! they link to. What is history (size-limited, cleared by "Clear cache"): recently read
//! articles, past Discover days, the HTTP and image caches.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::model::*;
use crate::settings::Settings;

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    pub dir: PathBuf,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS papers (key TEXT PRIMARY KEY, json TEXT NOT NULL, updated INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS articles (
    doc_key TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    json TEXT NOT NULL,
    created INTEGER NOT NULL,
    bytes INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS feed_items (
    id TEXT PRIMARY KEY,
    day TEXT NOT NULL,
    feed TEXT NOT NULL,
    tab TEXT NOT NULL,
    position INTEGER NOT NULL,
    json TEXT NOT NULL,
    bytes INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS feed_by_tab ON feed_items(feed, tab, day DESC, position);
CREATE TABLE IF NOT EXISTS feed_runs (day TEXT NOT NULL, feed TEXT NOT NULL, tab TEXT NOT NULL, done INTEGER NOT NULL, PRIMARY KEY(day, feed, tab));
CREATE TABLE IF NOT EXISTS saved (
    list TEXT NOT NULL,
    doc_key TEXT NOT NULL,
    doc TEXT NOT NULL,
    added INTEGER NOT NULL,
    PRIMARY KEY(list, doc_key)
);
CREATE TABLE IF NOT EXISTS history (doc_key TEXT PRIMARY KEY, doc TEXT NOT NULL, opened INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS bookshelves (id INTEGER PRIMARY KEY, name TEXT NOT NULL, project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL);
CREATE TABLE IF NOT EXISTS notes (
    id INTEGER PRIMARY KEY,
    project_id INTEGER REFERENCES projects(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    quotes TEXT NOT NULL DEFAULT '[]',
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS notifications (id INTEGER PRIMARY KEY, title TEXT NOT NULL, body TEXT NOT NULL, paper TEXT, created INTEGER NOT NULL, read INTEGER NOT NULL DEFAULT 0, dedupe TEXT UNIQUE);
CREATE TABLE IF NOT EXISTS queue (
    doc_key TEXT PRIMARY KEY,
    doc TEXT NOT NULL,
    status TEXT NOT NULL,
    error TEXT,
    added INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS asks (id INTEGER PRIMARY KEY, doc_key TEXT NOT NULL, question TEXT NOT NULL, answer TEXT NOT NULL, created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS http_cache (key TEXT PRIMARY KEY, body BLOB NOT NULL, fetched INTEGER NOT NULL);
"#;

fn ts(t: DateTime<Utc>) -> i64 {
    t.timestamp()
}
fn from_ts(t: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(t, 0).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueStatus {
    Queued,
    Running,
    Done,
    Failed,
}

impl QueueStatus {
    fn as_str(self) -> &'static str {
        match self {
            QueueStatus::Queued => "queued",
            QueueStatus::Running => "running",
            QueueStatus::Done => "done",
            QueueStatus::Failed => "failed",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "running" => QueueStatus::Running,
            "done" => QueueStatus::Done,
            "failed" => QueueStatus::Failed,
            _ => QueueStatus::Queued,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            QueueStatus::Queued => "Queued",
            QueueStatus::Running => "Writing…",
            QueueStatus::Done => "Ready",
            QueueStatus::Failed => "Failed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueueEntry {
    pub doc: Doc,
    pub status: QueueStatus,
    pub error: Option<String>,
    pub updated: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct SavedEntry {
    pub list: String,
    pub doc: Doc,
    pub added: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct AskEntry {
    pub id: i64,
    pub question: String,
    pub answer: Answer,
    pub created: DateTime<Utc>,
}

#[derive(Clone, Debug, Default)]
pub struct StorageUsage {
    pub history_articles: u64,
    pub saved_articles: u64,
    pub feed: u64,
    pub http_cache: u64,
    pub image_cache: u64,
}

impl StorageUsage {
    pub fn history_total(&self) -> u64 {
        self.history_articles + self.feed + self.http_cache + self.image_cache
    }
}

impl Db {
    /// Open (or create) the database in the platform data directory.
    // TODO(windows): `directories` gives %APPDATA% on Windows; check the path is writable.
    pub fn open_default() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("org", "GatelessScholar", "gateless").context("no home directory")?;
        let dir = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Self::open(&dir)
    }

    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir.join("images"))?;
        let conn = Connection::open(dir.join("gateless.sqlite3"))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Arc::new(Mutex::new(conn)), dir: dir.to_path_buf() })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("gateless-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("images"))?;
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db { conn: Arc::new(Mutex::new(conn)), dir })
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn image_cache_dir(&self) -> PathBuf {
        self.dir.join("images")
    }

    // ---------------------------------------------------------------- key/value + settings

    pub fn kv_get(&self, key: &str) -> Option<String> {
        self.c().query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
    }
    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.c().execute("INSERT INTO kv(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![key, value])?;
        Ok(())
    }
    pub fn load_settings(&self) -> Settings {
        self.kv_get("settings").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        self.kv_set("settings", &serde_json::to_string(s)?)
    }

    // ---------------------------------------------------------------- HTTP cache (research layer)

    /// Cached body and when it was fetched.
    pub fn cache_get(&self, key: &str) -> Option<(Vec<u8>, DateTime<Utc>)> {
        self.c()
            .query_row("SELECT body, fetched FROM http_cache WHERE key = ?1", [key], |r| Ok((r.get::<_, Vec<u8>>(0)?, from_ts(r.get(1)?))))
            .optional()
            .ok()
            .flatten()
    }
    pub fn cache_put(&self, key: &str, body: &[u8]) -> Result<()> {
        self.c().execute(
            "INSERT INTO http_cache(key, body, fetched) VALUES(?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET body = excluded.body, fetched = excluded.fetched",
            params![key, body, ts(Utc::now())],
        )?;
        Ok(())
    }

    // ---------------------------------------------------------------- papers

    pub fn put_paper(&self, p: &Paper) -> Result<()> {
        self.c().execute(
            "INSERT INTO papers(key, json, updated) VALUES(?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET json = excluded.json, updated = excluded.updated",
            params![p.key, serde_json::to_string(p)?, ts(Utc::now())],
        )?;
        Ok(())
    }
    pub fn get_paper(&self, key: &str) -> Option<Paper> {
        let s: Option<String> = self.c().query_row("SELECT json FROM papers WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }

    // ---------------------------------------------------------------- articles

    pub fn put_article(&self, doc_key: &str, a: &Article) -> Result<()> {
        let json = serde_json::to_string(a)?;
        self.c().execute(
            "INSERT INTO articles(doc_key, kind, title, json, created, bytes) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(doc_key) DO UPDATE SET kind = excluded.kind, title = excluded.title, json = excluded.json, created = excluded.created, bytes = excluded.bytes",
            params![doc_key, a.kind.as_str(), a.title, json, ts(a.created), json.len() as i64],
        )?;
        Ok(())
    }
    pub fn get_article(&self, doc_key: &str) -> Option<Article> {
        let s: Option<String> = self.c().query_row("SELECT json FROM articles WHERE doc_key = ?1", [doc_key], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }
    /// Keys of every document with a written article.
    pub fn article_keys(&self) -> std::collections::HashSet<String> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT doc_key FROM articles") else { return Default::default() };
        st.query_map([], |r| r.get::<_, String>(0)).map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
    }
    pub fn has_article(&self, doc_key: &str) -> bool {
        self.c().query_row("SELECT 1 FROM articles WHERE doc_key = ?1", [doc_key], |_| Ok(())).optional().ok().flatten().is_some()
    }
    pub fn delete_article(&self, doc_key: &str) -> Result<()> {
        self.c().execute("DELETE FROM articles WHERE doc_key = ?1", [doc_key])?;
        Ok(())
    }

    // ---------------------------------------------------------------- feed

    pub fn put_feed_item(&self, item: &FeedItem, position: i64) -> Result<()> {
        let json = serde_json::to_string(item)?;
        self.c().execute(
            "INSERT INTO feed_items(id, day, feed, tab, position, json, bytes) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json, bytes = excluded.bytes",
            params![item.id, item.day.to_string(), item.feed.as_str(), item.tab, position, json, json.len() as i64],
        )?;
        Ok(())
    }
    pub fn get_feed_item(&self, id: &str) -> Option<FeedItem> {
        let s: Option<String> = self.c().query_row("SELECT json FROM feed_items WHERE id = ?1", [id], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }
    /// Feed items for a tab, newest day first, then by position. `offset`/`limit` page through
    /// past days for infinite scroll.
    pub fn feed_page(&self, feed: FeedKind, tab: &str, offset: usize, limit: usize) -> Vec<FeedItem> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT json FROM feed_items WHERE feed = ?1 AND tab = ?2 ORDER BY day DESC, position ASC LIMIT ?3 OFFSET ?4") else {
            return vec![];
        };
        st.query_map(params![feed.as_str(), tab, limit as i64, offset as i64], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).filter_map(|s| serde_json::from_str(&s).ok()).collect())
            .unwrap_or_default()
    }
    pub fn feed_count_for_day(&self, feed: FeedKind, tab: &str, day: NaiveDate) -> usize {
        self.c()
            .query_row("SELECT COUNT(*) FROM feed_items WHERE feed = ?1 AND tab = ?2 AND day = ?3", params![feed.as_str(), tab, day.to_string()], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }
    pub fn feed_run_done(&self, day: NaiveDate, feed: FeedKind, tab: &str) -> bool {
        self.c()
            .query_row("SELECT done FROM feed_runs WHERE day = ?1 AND feed = ?2 AND tab = ?3", params![day.to_string(), feed.as_str(), tab], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0)
            == 1
    }
    pub fn set_feed_run_done(&self, day: NaiveDate, feed: FeedKind, tab: &str) -> Result<()> {
        self.c().execute(
            "INSERT INTO feed_runs(day, feed, tab, done) VALUES(?1, ?2, ?3, 1) ON CONFLICT DO UPDATE SET done = 1",
            params![day.to_string(), feed.as_str(), tab],
        )?;
        Ok(())
    }
    /// Paper keys already shown in a tab on recent days (so the daily feed brings new items).
    pub fn recent_feed_paper_keys(&self, feed: FeedKind, tab: &str, since: NaiveDate) -> Vec<String> {
        self.feed_page(feed, tab, 0, 2000)
            .into_iter()
            .filter(|f| f.day >= since)
            .flat_map(|f| f.paper.map(|p| p.key).into_iter().chain(f.basis.into_iter().map(|p| p.key)))
            .collect()
    }
    pub fn prune_feed_days(&self, keep_days: u32) -> Result<()> {
        let cutoff = Utc::now().date_naive() - chrono::Days::new(keep_days as u64);
        self.c().execute("DELETE FROM feed_items WHERE day < ?1", [cutoff.to_string()])?;
        self.c().execute("DELETE FROM feed_runs WHERE day < ?1", [cutoff.to_string()])?;
        Ok(())
    }

    // ---------------------------------------------------------------- saved lists

    pub fn save_doc(&self, list: SavedList, doc: &Doc) -> Result<()> {
        self.c().execute(
            "INSERT INTO saved(list, doc_key, doc, added) VALUES(?1, ?2, ?3, ?4) ON CONFLICT DO UPDATE SET doc = excluded.doc",
            params![list.key(), doc.key(), serde_json::to_string(doc)?, ts(Utc::now())],
        )?;
        if let Some(p) = doc.paper() {
            self.put_paper(p)?;
        }
        Ok(())
    }
    pub fn unsave_doc(&self, list: SavedList, doc_key: &str) -> Result<()> {
        self.c().execute("DELETE FROM saved WHERE list = ?1 AND doc_key = ?2", params![list.key(), doc_key])?;
        Ok(())
    }
    pub fn is_saved(&self, list: SavedList, doc_key: &str) -> bool {
        self.c()
            .query_row("SELECT 1 FROM saved WHERE list = ?1 AND doc_key = ?2", params![list.key(), doc_key], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }
    pub fn saved_lists_of(&self, doc_key: &str) -> Vec<String> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT list FROM saved WHERE doc_key = ?1") else { return vec![] };
        st.query_map([doc_key], |r| r.get::<_, String>(0)).map(|r| r.filter_map(|x| x.ok()).collect()).unwrap_or_default()
    }
    pub fn saved(&self, list: SavedList) -> Vec<SavedEntry> {
        self.saved_by_key(&list.key())
    }
    fn saved_by_key(&self, list: &str) -> Vec<SavedEntry> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT list, doc, added FROM saved WHERE list = ?1 ORDER BY added DESC") else { return vec![] };
        st.query_map([list], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .filter_map(|(list, doc, added)| Some(SavedEntry { list, doc: serde_json::from_str(&doc).ok()?, added: from_ts(added) }))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn is_permanent(&self, doc_key: &str) -> bool {
        !self.saved_lists_of(doc_key).is_empty()
    }

    // ---------------------------------------------------------------- history

    pub fn record_history(&self, doc: &Doc) -> Result<()> {
        self.c().execute(
            "INSERT INTO history(doc_key, doc, opened) VALUES(?1, ?2, ?3) ON CONFLICT(doc_key) DO UPDATE SET doc = excluded.doc, opened = excluded.opened",
            params![doc.key(), serde_json::to_string(doc)?, ts(Utc::now())],
        )?;
        if let Some(p) = doc.paper() {
            self.put_paper(p)?;
        }
        Ok(())
    }
    pub fn history(&self, limit: usize) -> Vec<(Doc, DateTime<Utc>)> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT doc, opened FROM history ORDER BY opened DESC LIMIT ?1") else { return vec![] };
        st.query_map([limit as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| rows.filter_map(|r| r.ok()).filter_map(|(d, t)| Some((serde_json::from_str(&d).ok()?, from_ts(t)))).collect())
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- projects, bookshelves, notes

    pub fn projects(&self) -> Vec<Project> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT id, name, description, created FROM projects ORDER BY name COLLATE NOCASE") else { return vec![] };
        st.query_map([], |r| Ok(Project { id: r.get(0)?, name: r.get(1)?, description: r.get(2)?, created: from_ts(r.get(3)?) }))
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }
    pub fn add_project(&self, name: &str, description: &str) -> Result<i64> {
        let c = self.c();
        c.execute("INSERT INTO projects(name, description, created) VALUES(?1, ?2, ?3)", params![name, description, ts(Utc::now())])?;
        Ok(c.last_insert_rowid())
    }
    pub fn update_project(&self, p: &Project) -> Result<()> {
        self.c().execute("UPDATE projects SET name = ?2, description = ?3 WHERE id = ?1", params![p.id, p.name, p.description])?;
        Ok(())
    }
    pub fn delete_project(&self, id: i64) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM saved WHERE list = ?1", [SavedList::Project(id).key()])?;
        c.execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn bookshelves(&self) -> Vec<Bookshelf> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT id, name, project_id FROM bookshelves ORDER BY name COLLATE NOCASE") else { return vec![] };
        st.query_map([], |r| Ok(Bookshelf { id: r.get(0)?, name: r.get(1)?, project_id: r.get(2)? }))
            .map(|r| r.filter_map(|x| x.ok()).collect())
            .unwrap_or_default()
    }
    pub fn add_bookshelf(&self, name: &str, project_id: Option<i64>) -> Result<i64> {
        let c = self.c();
        c.execute("INSERT INTO bookshelves(name, project_id) VALUES(?1, ?2)", params![name, project_id])?;
        Ok(c.last_insert_rowid())
    }
    pub fn update_bookshelf(&self, b: &Bookshelf) -> Result<()> {
        self.c().execute("UPDATE bookshelves SET name = ?2, project_id = ?3 WHERE id = ?1", params![b.id, b.name, b.project_id])?;
        Ok(())
    }
    pub fn delete_bookshelf(&self, id: i64) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM saved WHERE list = ?1", [SavedList::Bookshelf(id).key()])?;
        c.execute("DELETE FROM bookshelves WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn notes(&self, project: Option<i64>) -> Vec<Note> {
        let c = self.c();
        let sql = "SELECT id, project_id, title, body, quotes, created, updated FROM notes WHERE (?1 IS NULL OR project_id = ?1) ORDER BY updated DESC";
        let Ok(mut st) = c.prepare(sql) else { return vec![] };
        st.query_map([project], |r| {
            Ok(Note {
                id: r.get(0)?,
                project_id: r.get(1)?,
                title: r.get(2)?,
                body: r.get(3)?,
                quotes: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                created: from_ts(r.get(5)?),
                updated: from_ts(r.get(6)?),
            })
        })
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }
    pub fn add_note(&self, project_id: Option<i64>, title: &str) -> Result<i64> {
        let c = self.c();
        let now = ts(Utc::now());
        c.execute("INSERT INTO notes(project_id, title, body, quotes, created, updated) VALUES(?1, ?2, '', '[]', ?3, ?3)", params![project_id, title, now])?;
        Ok(c.last_insert_rowid())
    }
    /// Save a note. Articles/papers linked in the body are saved permanently (list "note").
    pub fn save_note(&self, n: &Note) -> Result<()> {
        self.c().execute(
            "UPDATE notes SET project_id = ?2, title = ?3, body = ?4, quotes = ?5, updated = ?6 WHERE id = ?1",
            params![n.id, n.project_id, n.title, n.body, serde_json::to_string(&n.quotes)?, ts(Utc::now())],
        )?;
        for key in note_links(&n.body).into_iter().chain(n.quotes.iter().filter_map(|q| q.paper_key.clone().or(q.article_id.clone()))) {
            if let Some(doc) = self.find_doc(&key) {
                self.save_doc(SavedList::NoteLink, &doc)?;
            }
        }
        Ok(())
    }
    pub fn delete_note(&self, id: i64) -> Result<()> {
        self.c().execute("DELETE FROM notes WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Find a document snapshot anywhere it is stored (saved lists, history, feed, papers).
    pub fn find_doc(&self, doc_key: &str) -> Option<Doc> {
        let c = self.c();
        let from_saved: Option<String> = c.query_row("SELECT doc FROM saved WHERE doc_key = ?1 LIMIT 1", [doc_key], |r| r.get(0)).optional().ok().flatten();
        let from_hist = || -> Option<String> { c.query_row("SELECT doc FROM history WHERE doc_key = ?1", [doc_key], |r| r.get(0)).optional().ok().flatten() };
        let from_queue = || -> Option<String> { c.query_row("SELECT doc FROM queue WHERE doc_key = ?1", [doc_key], |r| r.get(0)).optional().ok().flatten() };
        if let Some(d) = from_saved.or_else(from_hist).or_else(from_queue).and_then(|s| serde_json::from_str(&s).ok()) {
            return Some(d);
        }
        drop(c);
        if let Some(id) = doc_key.strip_prefix("feed:") {
            return self.get_feed_item(id).map(Doc::Feed);
        }
        self.get_paper(doc_key).map(Doc::Paper)
    }

    // ---------------------------------------------------------------- notifications

    /// Add a notification unless one with the same dedupe key exists. Returns true if added.
    pub fn add_notification(&self, title: &str, body: &str, paper: Option<&Paper>, dedupe: &str) -> Result<bool> {
        let n = self.c().execute(
            "INSERT OR IGNORE INTO notifications(title, body, paper, created, dedupe) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![title, body, paper.map(|p| serde_json::to_string(p).unwrap_or_default()), ts(Utc::now()), dedupe],
        )?;
        Ok(n > 0)
    }
    pub fn notifications(&self, limit: usize) -> Vec<Notification> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT id, title, body, paper, created, read FROM notifications ORDER BY created DESC LIMIT ?1") else { return vec![] };
        st.query_map([limit as i64], |r| {
            Ok(Notification {
                id: r.get(0)?,
                title: r.get(1)?,
                body: r.get(2)?,
                paper: r.get::<_, Option<String>>(3)?.and_then(|s| serde_json::from_str(&s).ok()),
                created: from_ts(r.get(4)?),
                read: r.get::<_, i64>(5)? == 1,
            })
        })
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
    }
    pub fn unread_notifications(&self) -> usize {
        self.c().query_row("SELECT COUNT(*) FROM notifications WHERE read = 0", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }
    pub fn mark_notifications_read(&self) -> Result<()> {
        self.c().execute("UPDATE notifications SET read = 1", [])?;
        Ok(())
    }
    pub fn clear_notifications(&self) -> Result<()> {
        self.c().execute("DELETE FROM notifications", [])?;
        Ok(())
    }

    // ---------------------------------------------------------------- generation queue

    pub fn enqueue(&self, doc: &Doc) -> Result<()> {
        let now = ts(Utc::now());
        self.c().execute(
            "INSERT INTO queue(doc_key, doc, status, error, added, updated) VALUES(?1, ?2, 'queued', NULL, ?3, ?3)
             ON CONFLICT(doc_key) DO UPDATE SET status = CASE WHEN queue.status = 'done' THEN 'done' ELSE 'queued' END, error = NULL, updated = excluded.updated",
            params![doc.key(), serde_json::to_string(doc)?, now],
        )?;
        Ok(())
    }
    pub fn set_queue_status(&self, doc_key: &str, status: QueueStatus, error: Option<&str>) -> Result<()> {
        self.c().execute(
            "UPDATE queue SET status = ?2, error = ?3, updated = ?4 WHERE doc_key = ?1",
            params![doc_key, status.as_str(), error, ts(Utc::now())],
        )?;
        Ok(())
    }
    pub fn queue(&self) -> Vec<QueueEntry> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT doc, status, error, updated FROM queue ORDER BY added DESC") else { return vec![] };
        st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i64>(3)?)))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .filter_map(|(d, s, e, u)| Some(QueueEntry { doc: serde_json::from_str(&d).ok()?, status: QueueStatus::parse(&s), error: e, updated: from_ts(u) }))
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn next_queued(&self) -> Option<Doc> {
        let s: Option<String> = self.c().query_row("SELECT doc FROM queue WHERE status = 'queued' ORDER BY added ASC LIMIT 1", [], |r| r.get(0)).optional().ok().flatten();
        s.and_then(|s| serde_json::from_str(&s).ok())
    }
    /// Jobs left "running" by a previous session go back to the queue.
    pub fn reset_running(&self) -> Result<()> {
        self.c().execute("UPDATE queue SET status = 'queued' WHERE status = 'running'", [])?;
        Ok(())
    }
    pub fn remove_from_queue(&self, doc_key: &str) -> Result<()> {
        self.c().execute("DELETE FROM queue WHERE doc_key = ?1", [doc_key])?;
        Ok(())
    }
    pub fn clear_finished_queue(&self) -> Result<()> {
        self.c().execute("DELETE FROM queue WHERE status IN ('done', 'failed')", [])?;
        Ok(())
    }

    // ---------------------------------------------------------------- ask panel

    pub fn add_ask(&self, doc_key: &str, question: &str, answer: &Answer) -> Result<()> {
        self.c().execute(
            "INSERT INTO asks(doc_key, question, answer, created) VALUES(?1, ?2, ?3, ?4)",
            params![doc_key, question, serde_json::to_string(answer)?, ts(Utc::now())],
        )?;
        Ok(())
    }
    pub fn asks(&self, doc_key: &str) -> Vec<AskEntry> {
        let c = self.c();
        let Ok(mut st) = c.prepare("SELECT id, question, answer, created FROM asks WHERE doc_key = ?1 ORDER BY created ASC") else { return vec![] };
        st.query_map([doc_key], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))
            .map(|rows| {
                rows.filter_map(|r| r.ok())
                    .filter_map(|(id, q, a, t)| Some(AskEntry { id, question: q, answer: serde_json::from_str(&a).ok()?, created: from_ts(t) }))
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- storage limits

    pub fn usage(&self) -> StorageUsage {
        let c = self.c();
        let sum = |sql: &str| -> u64 { c.query_row(sql, [], |r| r.get::<_, Option<i64>>(0)).ok().flatten().unwrap_or(0).max(0) as u64 };
        let history_articles = sum("SELECT SUM(bytes) FROM articles WHERE doc_key NOT IN (SELECT doc_key FROM saved)");
        let saved_articles = sum("SELECT SUM(bytes) FROM articles WHERE doc_key IN (SELECT doc_key FROM saved)");
        let feed = sum("SELECT SUM(bytes) FROM feed_items");
        let http_cache = sum("SELECT SUM(LENGTH(body)) FROM http_cache");
        drop(c);
        let image_cache = dir_size(&self.image_cache_dir());
        StorageUsage { history_articles, saved_articles, feed, http_cache, image_cache }
    }

    /// Keep history within `limit_bytes`: drop the HTTP cache first (it can be refetched),
    /// then cached images, then the oldest unsaved articles, then the oldest feed days.
    pub fn enforce_history_limit(&self, limit_bytes: u64) -> Result<()> {
        let mut u = self.usage();
        if u.history_total() <= limit_bytes {
            return Ok(());
        }
        // Oldest HTTP cache entries first.
        while u.history_total() > limit_bytes && u.http_cache > 0 {
            self.c().execute("DELETE FROM http_cache WHERE key IN (SELECT key FROM http_cache ORDER BY fetched ASC LIMIT 200)", [])?;
            u = self.usage();
        }
        if u.history_total() > limit_bytes {
            prune_dir_oldest(&self.image_cache_dir(), u.image_cache.saturating_sub(limit_bytes.saturating_sub(u.history_total() - u.image_cache)));
            u = self.usage();
        }
        while u.history_total() > limit_bytes && u.history_articles > 0 {
            self.c().execute(
                "DELETE FROM articles WHERE doc_key IN (SELECT doc_key FROM articles WHERE doc_key NOT IN (SELECT doc_key FROM saved) ORDER BY created ASC LIMIT 5)",
                [],
            )?;
            u = self.usage();
        }
        while u.history_total() > limit_bytes && u.feed > 0 {
            let oldest: Option<String> = self.c().query_row("SELECT MIN(day) FROM feed_items", [], |r| r.get(0)).optional()?.flatten();
            let Some(day) = oldest else { break };
            if day == Utc::now().date_naive().to_string() {
                break; // never drop today's feed
            }
            self.c().execute("DELETE FROM feed_items WHERE day = ?1", [day])?;
            u = self.usage();
        }
        Ok(())
    }

    /// "Clear cache": clears history only — never saved items (favourites, read later,
    /// bookshelves, projects, note links) or their articles. Deletes unsaved AI-written
    /// articles, past feed days, the HTTP cache and cached images.
    pub fn clear_cache(&self) -> Result<()> {
        let c = self.c();
        c.execute("DELETE FROM history", [])?;
        c.execute("DELETE FROM articles WHERE doc_key NOT IN (SELECT doc_key FROM saved)", [])?;
        c.execute("DELETE FROM feed_items WHERE day < ?1", [Utc::now().date_naive().to_string()])?;
        c.execute("DELETE FROM http_cache", [])?;
        c.execute("DELETE FROM asks WHERE doc_key NOT IN (SELECT doc_key FROM saved)", [])?;
        c.execute("DELETE FROM queue WHERE status IN ('done', 'failed') AND doc_key NOT IN (SELECT doc_key FROM saved)", [])?;
        drop(c);
        if let Ok(rd) = std::fs::read_dir(self.image_cache_dir()) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        self.c().execute_batch("VACUUM")?;
        Ok(())
    }
}

/// Keys linked in a note body: `[[article:<key>|Title]]` / `[[paper:<key>|Title]]`.
pub fn note_links(body: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let target = inner.split('|').next().unwrap_or("");
        if let Some(k) = target.strip_prefix("article:").or_else(|| target.strip_prefix("paper:")) {
            out.push(k.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
}

fn prune_dir_oldest(dir: &Path, mut to_free: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.metadata().ok()?.len(), e.path()))).collect();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if to_free == 0 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            to_free = to_free.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_items_survive_clear_cache() {
        let db = Db::open_in_memory().unwrap();
        let p = Paper { key: "doi:10.1/x".into(), title: "T".into(), ..Default::default() };
        let doc = Doc::Paper(p);
        db.save_doc(SavedList::Favourites, &doc).unwrap();
        let a = Article { id: "a".into(), title: "A".into(), ..Default::default() };
        db.put_article(&doc.key(), &a).unwrap();
        db.put_article("doi:10.1/other", &a).unwrap();
        db.record_history(&doc).unwrap();
        db.clear_cache().unwrap();
        assert!(db.has_article("doi:10.1/x"));
        assert!(!db.has_article("doi:10.1/other"));
        assert!(db.history(10).is_empty());
        assert!(db.is_saved(SavedList::Favourites, "doi:10.1/x"));
    }

    #[test]
    fn note_links_are_parsed() {
        assert_eq!(note_links("see [[paper:doi:10.1/x|Title]] and [[article:feed:abc|B]]"), vec!["doi:10.1/x", "feed:abc"]);
    }
}
