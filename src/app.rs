//! Application state, the event bus from background tasks, and the per-frame loop.
//!
//! Background work (research, writing, images, exports) runs on a tokio runtime and reports
//! back through `AppEvent`s; the UI never blocks on the network. Widgets don't mutate app state
//! directly while drawing: they push `Action`s, which are applied at the end of the frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use chrono::Utc;
use egui::{Color32, ColorImage, TextureHandle};

use crate::db::{AskEntry, Db};
use crate::jobs;
use crate::model::*;
use crate::providers::detect::Detected;
use crate::providers::{self, ProviderConfig, SharedProvider, Task};
use crate::research::{Research, ResearchConfig};
use crate::settings::{Settings, ThemePref};
use crate::ui::theme::{self, Palette};
use crate::writing::{Curated, SearchPlan};

// ---------------------------------------------------------------------------------------------
// Events from background tasks
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

pub enum AppEvent {
    Toast(String, ToastKind),
    DailyStatus { running: bool, text: String, done: usize, total: usize },
    FeedItem(FeedItem),
    FeedMore { feed: FeedKind, tab: String, items: Vec<FeedItem>, exhausted: bool },
    Search { id: u64, result: Result<Vec<Paper>, String>, append: bool },
    AiPlan { id: u64, plan: SearchPlan },
    AiCurated { id: u64, result: Result<Curated, String> },
    GenStage { key: String, text: String, fraction: f32 },
    GenSources { key: String, sources: Vec<Source> },
    GenDelta { key: String, text: String },
    GenSection { key: String, section: Section },
    GenDone { key: String, article: Box<Article> },
    GenFailed { key: String, error: String },
    AskDelta { key: String, text: String },
    AskDone { key: String, entry: AskEntry },
    AskFailed { key: String, error: String },
    Image { url: String, result: Result<ColorImage, String> },
    Detected(Detected),
    OllamaModels { models: Vec<String> },
    Notified(usize),
    ProviderTest { id: String, result: Result<String, String> },
    RelayInfo { id: String, result: Result<crate::relay::Info, String> },
    FoundImage { key: String, image: Option<ImageRef> },
}

/// Sends events to the UI thread and wakes it.
#[derive(Clone)]
pub struct Bus {
    tx: Sender<AppEvent>,
    ctx: egui::Context,
}

impl Bus {
    pub fn send(&self, ev: AppEvent) {
        let _ = self.tx.send(ev);
        self.ctx.request_repaint();
    }
    pub fn toast(&self, text: impl Into<String>, kind: ToastKind) {
        self.send(AppEvent::Toast(text.into(), kind));
    }
}

// ---------------------------------------------------------------------------------------------
// Views and actions
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Discover,
    Search,
    Reader(String),
    Saved(SavedList),
    Bookshelves,
    Projects(Option<i64>),
    Notes(Option<i64>),
    History,
    Queue,
    Notifications,
    Bibliography,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Txt,
    Docx,
    Pdf,
    Print,
    Email,
}

/// Things widgets ask for while drawing; applied after the frame's UI is built.
#[derive(Clone, Debug)]
pub enum Action {
    Open(Doc),
    OpenKey(String),
    Navigate(View),
    Back,
    Forward,
    Search(String),
    ToggleSave(SavedList, Doc),
    ReadLater(Doc),
    NewBookshelfWith(Doc),
    NewProjectWith(Doc),
    LinkInNote(Doc, Option<i64>),
    QuoteToNote { quote: NoteQuote, note: Option<i64> },
    ShowImage(ImageRef),
    LoadImage(String),
    OpenUrl(String),
    Copy(String),
    Export(ExportKind, String),
    Regenerate(String),
    ConfirmRegenerate(String),
    Generate(Doc),
    Ask(String, String),
    /// Learn AI: explain the abstract (`abstract_only`) or the whole article of a document.
    Learn { doc_key: String, abstract_only: bool },
    /// Find a photo for a card that has none (Wikipedia / Commons, cached).
    FindImage(Doc),
    Toast(String, ToastKind),
    RunDaily(bool),
    LoadMoreFeed,
    ToggleTheme,
    ToggleSidebar,
    SaveSettings,
}

// ---------------------------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------------------------

pub enum ImgState {
    Loading,
    Ready(TextureHandle),
    Failed,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: Instant,
}

#[derive(Default)]
pub struct DailyState {
    pub running: bool,
    pub text: String,
    pub done: usize,
    pub total: usize,
    pub cancel: Arc<AtomicBool>,
    pub last_checked_day: Option<chrono::NaiveDate>,
}

pub struct DiscoverState {
    pub feed: FeedKind,
    pub tab: usize,
    pub items: Vec<FeedItem>,
    pub db_offset: usize,
    pub db_exhausted: bool,
    pub more_page: u32,
    pub more_exhausted: bool,
    pub loading_more: bool,
    pub expanded: HashSet<String>,
    pub needs_reload: bool,
    pub renaming_tab: Option<(usize, String)>,
    pub new_tab: Option<String>,
    pub dragging_tab: Option<usize>,
}

impl Default for DiscoverState {
    fn default() -> Self {
        DiscoverState {
            feed: FeedKind::Research,
            tab: 0,
            items: vec![],
            db_offset: 0,
            db_exhausted: false,
            more_page: 0,
            more_exhausted: false,
            loading_more: false,
            expanded: HashSet::new(),
            needs_reload: true,
            renaming_tab: None,
            new_tab: None,
            dragging_tab: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SearchMode {
    #[default]
    Scholar,
    Ai,
}

#[derive(Default)]
pub struct SearchState {
    pub input: String,
    pub query: SearchQuery,
    pub mode: SearchMode,
    pub id: u64,
    pub results: Vec<Paper>,
    pub loading: bool,
    pub exhausted: bool,
    pub error: Option<String>,
    pub show_filters: bool,
    pub plan: Option<SearchPlan>,
    pub curated: Option<Curated>,
    pub curating: bool,
    pub expanded: HashSet<String>,
    pub focus_request: bool,
}

/// In-progress generation of one document's article.
pub struct GenState {
    pub text: String,
    pub fraction: f32,
    pub started: Instant,
    pub sources: Vec<Source>,
    pub sections: Vec<Section>,
    pub delta: String,
    pub error: Option<String>,
    pub done: bool,
}

pub struct ReaderState {
    pub doc: Doc,
    pub article: Option<Article>,
    pub ask_open: bool,
    pub ask_input: String,
    pub asks: Vec<AskEntry>,
    pub asking: Option<(String, String)>,
    pub ask_error: Option<String>,
    pub abstract_open: bool,
    pub pinned_original: Option<String>,
    /// Show the Learn AI (plain-language) version instead of the article.
    pub learn_mode: bool,
}

pub enum Modal {
    ClearCache,
    NewBookshelf { name: String, doc: Option<Doc> },
    NewProject { name: String, description: String, doc: Option<Doc> },
    Shortcuts,
    Welcome,
    ConfirmRegenerate(String),
}

#[derive(Default)]
pub struct NotesState {
    pub selected: Option<i64>,
    pub editing: Option<Note>,
    pub dirty: bool,
    pub last_edit: Option<Instant>,
}

#[derive(Default)]
pub struct SettingsUi {
    pub section: usize,
    pub api_key_inputs: HashMap<String, String>,
    pub new_provider: Option<providers::ProviderKind>,
    pub ollama_models: Vec<String>,
    pub tests: HashMap<String, Result<String, String>>,
    pub testing: HashSet<String>,
    pub new_feed: String,
    pub new_follow: String,
    pub new_follow_author: bool,
    pub core_key_input: String,
    pub s2_key_input: String,
    pub openalex_key_input: String,
    pub usage: Option<crate::db::StorageUsage>,
    pub new_tab: String,
    /// "Use my PC": models offered by the PC, per provider config id.
    pub relay_models: HashMap<String, crate::relay::Info>,
}

pub struct App {
    pub db: Db,
    pub settings: Settings,
    pub pal: Palette,
    pub rt: tokio::runtime::Handle,
    pub bus: Bus,
    rx: Receiver<AppEvent>,
    pub research: Research,
    /// Who draws the window frame (desktop) — always `Native` on Android.
    pub frame: craft_window::policy::Frame,

    pub view: View,
    pub back: Vec<View>,
    pub forward: Vec<View>,
    pub actions: Vec<Action>,

    pub discover: DiscoverState,
    pub daily: DailyState,
    pub search: SearchState,
    pub readers: HashMap<String, ReaderState>,
    pub gens: HashMap<String, GenState>,
    pub images: HashMap<String, ImgState>,
    pub toasts: Vec<Toast>,
    pub image_viewer: Option<ImageRef>,
    pub modal: Option<Modal>,
    pub notes: NotesState,
    pub settings_ui: SettingsUi,
    pub provider_cache: HashMap<String, SharedProvider>,
    pub detected: Option<Detected>,
    pub sidebar_sections: HashSet<&'static str>,
    pub unread_notifications: usize,
    pub bib_project: Option<i64>,
    pub last_follow_check: Option<Instant>,
    pub last_housekeeping: Option<Instant>,
    pub queue_running: Option<String>,
    pub search_has_focus: bool,
    pub saved_snapshot_version: u64,
    /// Learn AI articles by learn key ("learn:<doc key>" / "learn-abs:<doc key>").
    pub learn: HashMap<String, Article>,
    /// Photos found for documents that had none (doc key → image, None = nothing found).
    pub found_images: HashMap<String, Option<ImageRef>>,
    pub image_jobs: Arc<tokio::sync::Semaphore>,
    /// "Use this PC from my phone" server, while enabled.
    pub relay: Option<crate::relay::Server>,
    pub relay_error: Option<String>,
    /// Phone-sized window: drawer instead of sidebar, compact bars and cards.
    pub mobile: bool,
    /// The slide-out navigation drawer (phone layout).
    pub drawer_open: bool,
    /// Whether we last asked Android to show the on-screen keyboard.
    pub keyboard_shown: bool,
    keyboard_off_since: Option<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, db: Db, rt: tokio::runtime::Handle, frame: craft_window::policy::Frame) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        let settings = db.load_settings();
        let pal = Palette::for_pref(settings.theme);
        theme::apply(&cc.egui_ctx, &pal, settings.ui_scale);
        let (tx, rx) = std::sync::mpsc::channel();
        let bus = Bus { tx, ctx: cc.egui_ctx.clone() };
        let research = Research::new(db.clone(), research_config(&settings));
        let _ = db.reset_running();
        let unread = db.unread_notifications();
        let mut app = App {
            db,
            pal,
            rt,
            bus,
            rx,
            research,
            frame,
            view: View::Discover,
            back: vec![],
            forward: vec![],
            actions: vec![],
            discover: DiscoverState::default(),
            daily: DailyState::default(),
            search: SearchState { query: SearchQuery { per_page: 20, ..Default::default() }, ..Default::default() },
            readers: HashMap::new(),
            gens: HashMap::new(),
            images: HashMap::new(),
            toasts: vec![],
            image_viewer: None,
            modal: None,
            notes: NotesState::default(),
            settings_ui: SettingsUi::default(),
            provider_cache: HashMap::new(),
            detected: None,
            sidebar_sections: ["Library", "Research", "Activity"].into_iter().collect(),
            unread_notifications: unread,
            bib_project: None,
            last_follow_check: None,
            last_housekeeping: None,
            queue_running: None,
            search_has_focus: false,
            saved_snapshot_version: 0,
            learn: HashMap::new(),
            found_images: HashMap::new(),
            image_jobs: Arc::new(tokio::sync::Semaphore::new(3)),
            relay: None,
            relay_error: None,
            mobile: false,
            drawer_open: false,
            keyboard_shown: false,
            keyboard_off_since: None,
            settings,
        };
        if !app.settings.welcomed {
            app.modal = Some(Modal::Welcome);
        }
        jobs::detect_providers(&app);
        app.sync_relay();
        // Command line: `gateless --open <doc key>` / `gateless --search <query>`.
        let args: Vec<String> = std::env::args().collect();
        for w in args.windows(2) {
            match w[0].as_str() {
                "--open" => match app.db.find_doc(&w[1]) {
                    Some(doc) => app.actions.push(Action::Open(doc)),
                    None => app.toast(format!("Nothing stored under {}", w[1]), ToastKind::Error),
                },
                "--search" => app.actions.push(Action::Search(w[1].clone())),
                "--view" => {
                    let v = match w[1].as_str() {
                        "search" => Some(View::Search),
                        "favourites" => Some(View::Saved(SavedList::Favourites)),
                        "read-later" => Some(View::Saved(SavedList::ReadLater)),
                        "bookshelves" => Some(View::Bookshelves),
                        "projects" => Some(View::Projects(None)),
                        "notes" => Some(View::Notes(None)),
                        "history" => Some(View::History),
                        "queue" => Some(View::Queue),
                        "notifications" => Some(View::Notifications),
                        "bibliography" => Some(View::Bibliography),
                        "settings" => Some(View::Settings),
                        "news" => {
                            app.discover.feed = FeedKind::News;
                            Some(View::Discover)
                        }
                        _ => None,
                    };
                    if let Some(v) = v {
                        app.actions.push(Action::Navigate(v));
                    }
                }
                _ => {}
            }
        }
        if app.actions.iter().any(|a| matches!(a, Action::Open(_) | Action::Search(_) | Action::Navigate(_))) {
            app.modal = None;
        }
        app
    }

    // ------------------------------------------------------------------ providers

    pub fn provider_config(&self, task: Task) -> Option<ProviderConfig> {
        self.settings.provider_for(task).cloned()
    }

    pub fn provider(&mut self, task: Task) -> Option<SharedProvider> {
        let cfg = self.provider_config(task)?;
        if let Some(p) = self.provider_cache.get(&cfg.id) {
            return Some(p.clone());
        }
        match providers::build(&cfg) {
            Ok(p) => {
                self.provider_cache.insert(cfg.id.clone(), p.clone());
                Some(p)
            }
            Err(e) => {
                self.toast(format!("{}: {e}", cfg.label), ToastKind::Error);
                None
            }
        }
    }

    /// Brand of the article-writing provider for bylines ("Claude").
    pub fn provider_brand(&self) -> String {
        self.provider_config(Task::Articles).map(|c| c.kind.brand().to_string()).unwrap_or_else(|| "AI".into())
    }

    pub fn has_provider(&self) -> bool {
        self.settings.providers.iter().any(|p| p.enabled)
    }

    pub fn settings_changed(&mut self) {
        self.provider_cache.clear();
        self.sync_relay();
        self.research = Research::new(self.db.clone(), research_config(&self.settings));
        if let Err(e) = self.db.save_settings(&self.settings) {
            self.toast(format!("Could not save settings: {e}"), ToastKind::Error);
        }
    }

    /// Start, stop or update the "Use my PC" relay to match settings (desktop only).
    pub fn sync_relay(&mut self) {
        if crate::platform::is_android() || !self.settings.relay_enabled {
            self.relay = None;
            self.relay_error = None;
            return;
        }
        let offer = crate::relay::Offer {
            // Never offer a relay through a relay.
            providers: self.settings.providers.iter().filter(|p| p.kind != providers::ProviderKind::Relay).cloned().collect(),
            default_id: self.settings.provider_for(Task::Articles).filter(|p| p.kind != providers::ProviderKind::Relay).map(|p| p.id.clone()),
        };
        let code = match providers::secrets::get_api_key("relay-pairing-code") {
            Ok(Some(c)) => c,
            _ => {
                let c = crate::relay::new_pairing_code();
                let _ = providers::secrets::set_api_key("relay-pairing-code", &c);
                c
            }
        };
        match &self.relay {
            Some(r) if r.port == self.settings.relay_port && r.code == code => {
                if let Ok(mut o) = r.offer.write() {
                    *o = offer;
                }
            }
            _ => {
                self.relay = None;
                match crate::relay::start(&self.rt, self.settings.relay_port, code, offer) {
                    Ok(s) => {
                        self.relay = Some(s);
                        self.relay_error = None;
                    }
                    Err(e) => self.relay_error = Some(format!("{e:#}")),
                }
            }
        }
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, pref: ThemePref) {
        self.settings.theme = pref;
        self.pal = Palette::for_pref(pref);
        theme::apply(ctx, &self.pal, self.settings.ui_scale);
        self.settings_changed();
    }

    // ------------------------------------------------------------------ navigation

    pub fn navigate(&mut self, view: View) {
        self.drawer_open = false;
        if self.view != view {
            let prev = std::mem::replace(&mut self.view, view);
            self.back.push(prev);
            self.forward.clear();
            if self.back.len() > 100 {
                self.back.remove(0);
            }
        }
        if matches!(self.view, View::Notifications) {
            let _ = self.db.mark_notifications_read();
            self.unread_notifications = 0;
        }
        if matches!(self.view, View::Settings) {
            self.settings_ui.usage = Some(self.db.usage());
        }
    }

    pub fn go_back(&mut self) {
        if let Some(v) = self.back.pop() {
            let cur = std::mem::replace(&mut self.view, v);
            self.forward.push(cur);
        }
    }
    pub fn go_forward(&mut self) {
        if let Some(v) = self.forward.pop() {
            let cur = std::mem::replace(&mut self.view, v);
            self.back.push(cur);
        }
    }

    /// Open a document in the reader: abstract immediately, saved article if there is one,
    /// otherwise start writing it.
    pub fn open_doc(&mut self, doc: Doc) {
        let key = doc.key();
        let _ = self.db.record_history(&doc);
        if !self.readers.contains_key(&key) {
            let article = self.db.get_article(&key);
            let asks = self.db.asks(&key);
            self.readers.insert(
                key.clone(),
                ReaderState { doc: doc.clone(), article, ask_open: false, ask_input: String::new(), asks, asking: None, ask_error: None, abstract_open: true, pinned_original: None, learn_mode: false },
            );
        }
        for lk in [learn_key(&key, false), learn_key(&key, true)] {
            if !self.learn.contains_key(&lk)
                && let Some(a) = self.db.get_article(&lk)
            {
                self.learn.insert(lk, a);
            }
        }
        let has_article = self.readers.get(&key).is_some_and(|r| r.article.is_some());
        if !has_article && !self.gens.contains_key(&key) {
            self.start_generation(doc, false);
        }
        self.navigate(View::Reader(key));
    }

    pub fn start_generation(&mut self, doc: Doc, from_queue: bool) {
        let key = doc.key();
        if self.gens.get(&key).is_some_and(|g| !g.done && g.error.is_none()) {
            return;
        }
        let task = if doc.kind() == ContentKind::News { Task::News } else { Task::Articles };
        let Some(provider) = self.provider(task) else {
            self.gens.insert(
                key.clone(),
                GenState {
                    text: String::new(),
                    fraction: 0.0,
                    started: Instant::now(),
                    sources: vec![],
                    sections: vec![],
                    delta: String::new(),
                    error: Some("No AI model is set up yet. Add one in Settings › Models to have articles written.".into()),
                    done: false,
                },
            );
            if from_queue {
                let _ = self.db.set_queue_status(&key, crate::db::QueueStatus::Failed, Some("No AI model set up"));
            }
            return;
        };
        self.gens.insert(
            key.clone(),
            GenState { text: "Finding sources…".into(), fraction: 0.01, started: Instant::now(), sources: vec![], sections: vec![], delta: String::new(), error: None, done: false },
        );
        if from_queue {
            self.queue_running = Some(key.clone());
        }
        let _ = self.db.enqueue(&doc);
        let _ = self.db.set_queue_status(&key, crate::db::QueueStatus::Running, None);
        jobs::generate(self, doc, provider);
    }

    pub fn toast(&mut self, text: impl Into<String>, kind: ToastKind) {
        let secs = if kind == ToastKind::Error { 7 } else { 4 };
        let text = text.into();
        let until = Instant::now() + Duration::from_secs(secs);
        // The same message again (e.g. several background tasks hitting one problem): show it once.
        if let Some(t) = self.toasts.iter_mut().find(|t| t.text == text) {
            t.until = until;
            return;
        }
        self.toasts.push(Toast { text, kind, until });
    }

    pub fn request_image(&mut self, url: &str) {
        if url.is_empty() || self.images.contains_key(url) {
            return;
        }
        self.images.insert(url.to_string(), ImgState::Loading);
        jobs::load_image(self, url.to_string());
    }

    pub fn texture(&self, url: &str) -> Option<&TextureHandle> {
        match self.images.get(url) {
            Some(ImgState::Ready(t)) => Some(t),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ events

    fn handle_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                AppEvent::Toast(t, k) => self.toast(t, k),
                AppEvent::DailyStatus { running, text, done, total } => {
                    self.daily.running = running;
                    self.daily.text = text;
                    self.daily.done = done;
                    self.daily.total = total;
                    if !running {
                        self.discover.needs_reload = true;
                    }
                }
                AppEvent::FeedItem(item) => {
                    let tab = self.settings.tabs.get(self.discover.tab).cloned().unwrap_or_default();
                    if item.feed == self.discover.feed && item.tab == tab && !self.discover.items.iter().any(|i| i.id == item.id) {
                        // Today's items go before older days, in arrival order.
                        let pos = self.discover.items.iter().position(|i| i.day < item.day).unwrap_or(self.discover.items.len());
                        self.discover.items.insert(pos, item);
                        self.discover.db_offset += 1;
                    }
                }
                AppEvent::FeedMore { feed, tab, items, exhausted } => {
                    let cur = self.settings.tabs.get(self.discover.tab).cloned().unwrap_or_default();
                    if feed == self.discover.feed && tab == cur {
                        let known: HashSet<String> = self.discover.items.iter().map(|i| i.id.clone()).collect();
                        self.discover.items.extend(items.into_iter().filter(|i| !known.contains(&i.id)));
                        self.discover.more_exhausted = exhausted;
                        self.discover.more_page += 1;
                    }
                    self.discover.loading_more = false;
                }
                AppEvent::Search { id, result, append } => {
                    if id == self.search.id {
                        self.search.loading = false;
                        match result {
                            Ok(papers) => {
                                self.search.exhausted = papers.is_empty();
                                if append {
                                    let known: HashSet<String> = self.search.results.iter().map(|p| p.key.clone()).collect();
                                    self.search.results.extend(papers.into_iter().filter(|p| !known.contains(&p.key)));
                                } else {
                                    self.search.results = papers;
                                }
                                self.search.error = None;
                            }
                            Err(e) => self.search.error = Some(e),
                        }
                    }
                }
                AppEvent::AiPlan { id, plan } => {
                    if id == self.search.id {
                        self.search.plan = Some(plan);
                    }
                }
                AppEvent::AiCurated { id, result } => {
                    if id == self.search.id {
                        self.search.curating = false;
                        match result {
                            Ok(c) => self.search.curated = Some(c),
                            Err(e) => self.toast(format!("AI curation failed: {e}"), ToastKind::Error),
                        }
                    }
                }
                AppEvent::GenStage { key, text, fraction } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.text = text;
                        g.fraction = g.fraction.max(fraction);
                    }
                }
                AppEvent::GenSources { key, sources } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.sources = sources;
                    }
                }
                AppEvent::GenDelta { key, text } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.delta.push_str(&text);
                        if g.delta.len() > 6000 {
                            let cut = g.delta.len() - 4000;
                            let cut = (cut..g.delta.len()).find(|i| g.delta.is_char_boundary(*i)).unwrap_or(0);
                            g.delta.drain(..cut);
                        }
                    }
                }
                AppEvent::GenSection { key, section } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.sections.push(section);
                        g.delta.clear();
                    }
                }
                AppEvent::GenDone { key, article } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.done = true;
                    }
                    self.gens.remove(&key);
                    if key.starts_with("learn") {
                        self.learn.insert(key, *article);
                        continue;
                    }
                    if let Some(r) = self.readers.get_mut(&key) {
                        r.article = Some(*article.clone());
                    }
                    if self.queue_running.as_deref() == Some(key.as_str()) {
                        self.queue_running = None;
                    }
                    if !matches!(&self.view, View::Reader(k) if *k == key) {
                        self.toast(format!("Ready to read: {}", article.title), ToastKind::Success);
                    }
                }
                AppEvent::GenFailed { key, error } => {
                    if let Some(g) = self.gens.get_mut(&key) {
                        g.error = Some(error.clone());
                    }
                    if self.queue_running.as_deref() == Some(key.as_str()) {
                        self.queue_running = None;
                    }
                    if !key.starts_with("learn") && !matches!(&self.view, View::Reader(k) if *k == key) {
                        self.toast(format!("Writing failed: {error}"), ToastKind::Error);
                    }
                }
                AppEvent::AskDelta { key, text } => {
                    if let Some(r) = self.readers.get_mut(&key)
                        && let Some((_, partial)) = r.asking.as_mut()
                    {
                        partial.push_str(&text);
                    }
                }
                AppEvent::AskDone { key, entry } => {
                    if let Some(r) = self.readers.get_mut(&key) {
                        r.asking = None;
                        r.asks.push(entry);
                    }
                }
                AppEvent::AskFailed { key, error } => {
                    if let Some(r) = self.readers.get_mut(&key) {
                        r.asking = None;
                        r.ask_error = Some(error);
                    }
                }
                AppEvent::Image { url, result } => {
                    let state = match result {
                        Ok(img) => ImgState::Ready(ctx.load_texture(&url, img, egui::TextureOptions::LINEAR)),
                        Err(e) => {
                            log::debug!("image {url}: {e}");
                            ImgState::Failed
                        }
                    };
                    self.images.insert(url, state);
                }
                AppEvent::Detected(d) => {
                    self.settings_ui.ollama_models = d.ollama_models.clone();
                    self.detected = Some(d);
                }
                AppEvent::OllamaModels { models } => self.settings_ui.ollama_models = models,
                AppEvent::Notified(n) => {
                    self.unread_notifications = self.db.unread_notifications();
                    if n > 0 {
                        self.toast(format!("{n} new paper{} from topics and authors you follow", if n == 1 { "" } else { "s" }), ToastKind::Info);
                    }
                }
                AppEvent::FoundImage { key, image } => {
                    if let Some(i) = &image {
                        self.request_image(&i.thumb_url.clone().unwrap_or_else(|| i.url.clone()));
                    }
                    self.found_images.insert(key, image);
                }
                AppEvent::RelayInfo { id, result } => {
                    self.settings_ui.testing.remove(&id);
                    match result {
                        Ok(info) => {
                            self.settings_ui.tests.insert(id.clone(), Ok(format!("Connected to your PC — {} model(s) available", info.models.len())));
                            self.settings_ui.relay_models.insert(id, info);
                        }
                        Err(e) => {
                            self.settings_ui.tests.insert(id, Err(e));
                        }
                    }
                }
                AppEvent::ProviderTest { id, result } => {
                    self.settings_ui.testing.remove(&id);
                    self.settings_ui.tests.insert(id, result);
                }
            }
        }
    }

    // ------------------------------------------------------------------ actions

    fn apply_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        // Actions change state after this frame was drawn: draw again so the change shows
        // (on touch screens no further input event would trigger that frame).
        if !actions.is_empty() {
            ctx.request_repaint();
        }
        for a in actions {
            match a {
                Action::Open(doc) => self.open_doc(doc),
                Action::OpenKey(key) => match self.db.find_doc(&key) {
                    Some(doc) => self.open_doc(doc),
                    None => self.toast("That item is no longer stored.", ToastKind::Error),
                },
                Action::Navigate(v) => self.navigate(v),
                Action::Back => self.go_back(),
                Action::Forward => self.go_forward(),
                Action::Search(q) => {
                    self.search.input = q;
                    self.run_search(false);
                    self.navigate(View::Search);
                }
                Action::ToggleSave(list, doc) => {
                    let key = doc.key();
                    let r = if self.db.is_saved(list, &key) { self.db.unsave_doc(list, &key) } else { self.db.save_doc(list, &doc) };
                    if let Err(e) = r {
                        self.toast(e.to_string(), ToastKind::Error);
                    }
                    self.saved_snapshot_version += 1;
                }
                Action::ReadLater(doc) => {
                    let key = doc.key();
                    if self.db.is_saved(SavedList::ReadLater, &key) {
                        let _ = self.db.unsave_doc(SavedList::ReadLater, &key);
                    } else {
                        let _ = self.db.save_doc(SavedList::ReadLater, &doc);
                        if !self.db.has_article(&key) {
                            let _ = self.db.enqueue(&doc);
                            self.toast("Added to Read Later — the full article is queued for writing.", ToastKind::Success);
                        } else {
                            self.toast("Added to Read Later.", ToastKind::Success);
                        }
                    }
                    self.saved_snapshot_version += 1;
                }
                Action::NewBookshelfWith(doc) => self.modal = Some(Modal::NewBookshelf { name: String::new(), doc: Some(doc) }),
                Action::NewProjectWith(doc) => self.modal = Some(Modal::NewProject { name: String::new(), description: String::new(), doc: Some(doc) }),
                Action::LinkInNote(doc, note) => self.link_in_note(doc, note),
                Action::QuoteToNote { quote, note } => self.quote_to_note(quote, note),
                Action::ShowImage(img) => {
                    self.request_image(&img.url);
                    self.image_viewer = Some(img);
                }
                Action::LoadImage(url) => self.request_image(&url),
                Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
                Action::Copy(text) => {
                    ctx.copy_text(text);
                    self.toast("Copied", ToastKind::Info);
                }
                Action::Export(kind, key) => {
                    if let Some(article) = self.readers.get(&key).and_then(|r| r.article.clone()).or_else(|| self.db.get_article(&key)) {
                        jobs::export(self, kind, article);
                    } else {
                        self.toast("The article isn't written yet.", ToastKind::Error);
                    }
                }
                Action::Regenerate(key) => {
                    let _ = self.db.delete_article(&key);
                    if let Some(r) = self.readers.get_mut(&key) {
                        r.article = None;
                        let doc = r.doc.clone();
                        self.gens.remove(&key);
                        self.start_generation(doc, false);
                    }
                }
                Action::ConfirmRegenerate(key) => self.modal = Some(Modal::ConfirmRegenerate(key)),
                Action::Generate(doc) => {
                    self.gens.remove(&doc.key());
                    self.start_generation(doc, false);
                }
                Action::Ask(key, question) => self.ask(key, question),
                Action::Learn { doc_key, abstract_only } => self.start_learn(doc_key, abstract_only),
                Action::FindImage(doc) => {
                    let key = doc.key();
                    if !self.found_images.contains_key(&key) {
                        self.found_images.insert(key, None);
                        jobs::find_image(self, doc, self.current_tab());
                    }
                }
                Action::Toast(t, k) => self.toast(t, k),
                Action::RunDaily(force) => self.run_daily(force),
                Action::LoadMoreFeed => self.load_more_feed(),
                Action::ToggleTheme => {
                    let next = if self.settings.theme == ThemePref::Dark { ThemePref::Light } else { ThemePref::Dark };
                    self.set_theme(ctx, next);
                }
                Action::ToggleSidebar => {
                    self.settings.sidebar_open = !self.settings.sidebar_open;
                    self.settings_changed();
                }
                Action::SaveSettings => self.settings_changed(),
            }
        }
    }

    fn link_in_note(&mut self, doc: Doc, note: Option<i64>) {
        let id = match note {
            Some(id) => id,
            None => match self.db.add_note(None, &format!("Notes on {}", truncate(doc.title(), 60))) {
                Ok(id) => id,
                Err(e) => return self.toast(e.to_string(), ToastKind::Error),
            },
        };
        if let Some(mut n) = self.db.notes(None).into_iter().find(|n| n.id == id) {
            let kind = if doc.key().starts_with("feed:") { "article" } else { "paper" };
            if !n.body.is_empty() && !n.body.ends_with('\n') {
                n.body.push('\n');
            }
            n.body.push_str(&format!("[[{kind}:{}|{}]]\n", doc.key(), doc.title().replace(['[', ']', '|'], "")));
            let _ = self.db.save_doc(SavedList::NoteLink, &doc);
            if let Err(e) = self.db.save_note(&n) {
                self.toast(e.to_string(), ToastKind::Error);
            }
            self.notes.selected = Some(id);
            self.notes.editing = None;
            self.toast(format!("Linked in “{}” — the article is now kept permanently.", n.title), ToastKind::Success);
        }
    }

    fn quote_to_note(&mut self, quote: NoteQuote, note: Option<i64>) {
        let id = match note {
            Some(id) => id,
            None => match self.db.add_note(None, "Quotes") {
                Ok(id) => id,
                Err(e) => return self.toast(e.to_string(), ToastKind::Error),
            },
        };
        if let Some(mut n) = self.db.notes(None).into_iter().find(|n| n.id == id) {
            n.quotes.push(quote);
            if let Err(e) = self.db.save_note(&n) {
                self.toast(e.to_string(), ToastKind::Error);
            }
            self.notes.selected = Some(id);
            self.notes.editing = None;
            self.toast(format!("Quoted into “{}”", n.title), ToastKind::Success);
        }
    }

    /// Learn AI for a document: its abstract, or (when written) its whole article.
    fn start_learn(&mut self, doc_key: String, abstract_only: bool) {
        let lk = learn_key(&doc_key, abstract_only);
        if self.learn.contains_key(&lk) || self.gens.get(&lk).is_some_and(|g| g.error.is_none()) {
            if let Some(r) = self.readers.get_mut(&doc_key) && !abstract_only {
                r.learn_mode = true;
            }
            return;
        }
        let Some(r) = self.readers.get_mut(&doc_key) else { return };
        let input = if abstract_only {
            crate::writing::learn::LearnInput::from_abstract(&r.doc)
        } else {
            match &r.article {
                Some(a) => crate::writing::learn::LearnInput::from_article(a),
                None => return self.toast("Learn AI for the whole article works once it's written — try it on the abstract meanwhile.", ToastKind::Info),
            }
        };
        if !abstract_only {
            r.learn_mode = true;
        }
        let Some(provider) = self.provider(Task::Learn) else {
            self.toast("Learn AI needs an AI model — set one up in Settings › Models.", ToastKind::Error);
            return;
        };
        self.gens.insert(
            lk.clone(),
            GenState { text: "Explaining in plain words…".into(), fraction: 0.02, started: Instant::now(), sources: input.sources.clone(), sections: vec![], delta: String::new(), error: None, done: false },
        );
        jobs::learn(self, lk, input, provider);
    }

    fn ask(&mut self, key: String, question: String) {
        let question = question.trim().to_string();
        if question.is_empty() {
            return;
        }
        let Some(article) = self.readers.get(&key).and_then(|r| r.article.clone()) else {
            self.toast("Wait for the article to be written, then ask about it.", ToastKind::Info);
            return;
        };
        let Some(provider) = self.provider(Task::Ask) else {
            self.toast("Set up an AI model in Settings › Models to ask questions.", ToastKind::Error);
            return;
        };
        if let Some(r) = self.readers.get_mut(&key) {
            r.asking = Some((question.clone(), String::new()));
            r.ask_error = None;
            r.ask_input.clear();
        }
        jobs::ask(self, key, article, question, provider);
    }

    // ------------------------------------------------------------------ discover

    pub fn current_tab(&self) -> String {
        self.settings.tabs.get(self.discover.tab).cloned().unwrap_or_else(|| "For You".into())
    }

    pub fn reload_feed(&mut self) {
        let tab = self.current_tab();
        self.discover.items = self.db.feed_page(self.discover.feed, &tab, 0, 40);
        self.discover.db_offset = self.discover.items.len();
        self.discover.db_exhausted = self.discover.items.len() < 40;
        self.discover.more_page = 0;
        self.discover.more_exhausted = false;
        self.discover.loading_more = false;
        self.discover.needs_reload = false;
    }

    fn load_more_feed(&mut self) {
        if self.discover.loading_more {
            return;
        }
        let tab = self.current_tab();
        if !self.discover.db_exhausted {
            let more = self.db.feed_page(self.discover.feed, &tab, self.discover.db_offset, 30);
            self.discover.db_exhausted = more.len() < 30;
            self.discover.db_offset += more.len();
            let known: HashSet<String> = self.discover.items.iter().map(|i| i.id.clone()).collect();
            self.discover.items.extend(more.into_iter().filter(|i| !known.contains(&i.id)));
            return;
        }
        if self.discover.more_exhausted {
            return;
        }
        self.discover.loading_more = true;
        jobs::feed_more(self, self.discover.feed, tab, self.discover.more_page);
    }

    pub fn run_daily(&mut self, force: bool) {
        if self.daily.running {
            return;
        }
        let today = Utc::now().date_naive();
        self.daily.last_checked_day = Some(today);
        let mut tabs = self.settings.tabs.clone();
        // The tab being looked at goes first.
        let cur = self.current_tab();
        tabs.retain(|t| *t != cur);
        tabs.insert(0, cur);
        let headline_provider = self.provider(Task::Headlines);
        let news_provider = self.provider(Task::Headlines).or_else(|| self.provider(Task::News));
        self.daily.running = true;
        self.daily.text = "Preparing today's Discover…".into();
        self.daily.cancel = Arc::new(AtomicBool::new(false));
        jobs::daily(self, tabs, force, headline_provider, news_provider);
    }

    // ------------------------------------------------------------------ search

    pub fn run_search(&mut self, append: bool) {
        let text = self.search.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        if !append {
            self.search.id += 1;
            self.search.query.text = text.clone();
            self.search.query.page = 0;
            self.search.results.clear();
            self.search.curated = None;
            self.search.plan = None;
            self.search.exhausted = false;
            self.search.error = None;
        } else {
            if self.search.loading || self.search.exhausted {
                return;
            }
            self.search.query.page += 1;
        }
        self.search.loading = true;
        if self.search.mode == SearchMode::Ai && !append {
            match self.provider(Task::Search) {
                Some(p) => {
                    self.search.curating = true;
                    jobs::ai_search(self, self.search.id, self.search.query.clone(), p);
                    return;
                }
                None => self.toast("AI mode needs a model (Settings › Models) — showing Scholar results.", ToastKind::Info),
            }
        }
        jobs::search(self, self.search.id, self.search.query.clone(), append);
    }

    // ------------------------------------------------------------------ periodic work

    fn tick(&mut self) {
        // Daily Discover: once per day while open.
        let today = Utc::now().date_naive();
        if self.settings.auto_daily && self.settings.welcomed && self.daily.last_checked_day != Some(today) && !self.daily.running {
            self.run_daily(false);
        }
        // Generation queue: one queued article at a time in the background.
        if self.queue_running.is_none()
            && let Some(doc) = self.db.next_queued()
        {
            let key = doc.key();
            if self.db.has_article(&key) {
                let _ = self.db.set_queue_status(&key, crate::db::QueueStatus::Done, None);
            } else if self.gens.get(&key).is_some_and(|g| g.error.is_none()) {
                // Already being written (opened in the reader).
            } else if self.has_provider() {
                self.gens.remove(&key);
                self.start_generation(doc, true);
            }
        }
        // Follows: check for new papers every 30 minutes while the app is open.
        let due = self.last_follow_check.is_none_or(|t| t.elapsed() > Duration::from_secs(30 * 60));
        if due && !self.settings.follows.is_empty() {
            self.last_follow_check = Some(Instant::now());
            jobs::check_follows(self);
        }
        // Housekeeping: keep history within its size limit.
        if self.last_housekeeping.is_none_or(|t| t.elapsed() > Duration::from_secs(10 * 60)) {
            self.last_housekeeping = Some(Instant::now());
            let db = self.db.clone();
            let limit = self.settings.history_limit_mb * 1024 * 1024;
            let days = self.settings.feed_days_kept;
            self.rt.spawn_blocking(move || {
                let _ = db.prune_feed_days(days.max(7));
                let _ = db.enforce_history_limit(limit);
            });
        }
        self.toasts.retain(|t| t.until > Instant::now());
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events(&ctx);
        self.tick();
        if self.discover.needs_reload {
            self.reload_feed();
        }
        self.shortcuts(&ctx);
        self.draw(ui);
        self.apply_actions(&ctx);
        // Android: show the on-screen keyboard while a text field has focus (this egui does
        // not request it there itself). A tap briefly drops focus on touch-down, so only hide
        // once focus has been gone for a moment.
        if crate::platform::is_android() {
            let wants = ctx.egui_wants_keyboard_input();
            if wants {
                self.keyboard_off_since = None;
                if !self.keyboard_shown {
                    crate::platform::set_soft_keyboard(true);
                    self.keyboard_shown = true;
                }
                ctx.request_repaint_after(Duration::from_millis(250));
            } else if self.keyboard_shown {
                let since = *self.keyboard_off_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_millis(300) {
                    crate::platform::set_soft_keyboard(false);
                    self.keyboard_shown = false;
                    self.keyboard_off_since = None;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
        }
        if !self.toasts.is_empty() || self.daily.running || !self.gens.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else {
            ctx.request_repaint_after(Duration::from_secs(30));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.pal.bg;
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }
}

pub fn research_config(s: &Settings) -> ResearchConfig {
    ResearchConfig {
        contact_email: s.contact_email.trim().to_string(),
        core_api_key: providers::secrets::get_api_key("core-api-key").ok().flatten(),
        s2_api_key: providers::secrets::get_api_key("s2-api-key").ok().flatten(),
        news_feeds: s.news_feeds.clone(),
    }
}

/// Storage key of a document's Learn AI explanation.
pub fn learn_key(doc_key: &str, abstract_only: bool) -> String {
    if abstract_only { format!("learn-abs:{doc_key}") } else { format!("learn:{doc_key}") }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

pub fn toast_color(pal: &Palette, kind: ToastKind) -> Color32 {
    match kind {
        ToastKind::Info => pal.accent,
        ToastKind::Success => pal.link,
        ToastKind::Error => pal.danger,
    }
}
