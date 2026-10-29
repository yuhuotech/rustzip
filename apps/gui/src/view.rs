//! The main window: tabs, toolbar, folder list, details, tasks and status.
//!
//! All decisions (what a folder contains, how a task is doing) come from `rz-app`;
//! this file decides only where things go on screen.

use crate::brand::{self, palette};
use crate::dialog::{Dialog, Tone};
use crate::open_requests::OpenRequests;
use crate::recent::{self, RecentList, RecentSort};
use crate::rows::FileRows;
use crate::services::UpdateState;
use crate::{
    BlankAdd, CloseTab, ExtractAll, FocusSearch, NewArchiveDialog, OpenArchiveDialog, OpenSettings,
    RowAddHere, RowCopyPath, RowDelete, RowExtractHere, RowExtractNamed, RowExtractTo, RowOpen,
    RowRename, SelectAllRows,
};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::browse::{ArchiveTree, Row, RowKind, compression_ratio, format_size};
use rz_app::passwords::{MemoryStore, SecretStore};
use rz_app::settings::{OnConflict, Settings, Theme};
use rz_app::tasks::{ErrorCode, JobKind, JobSpec, TaskId, TaskInfo, TaskManager, TaskState};
use rz_core::{CreateOptions, Error};
use rz_formats::OutputFormat;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Whether the tabs share the title bar row with the window buttons (macOS only).
pub const TABS_IN_TITLEBAR: bool = cfg!(target_os = "macos");
const TITLEBAR_HEIGHT: f32 = 38.;
const EMAIL_SHARE_VOLUME_SIZE: u64 = 20 << 20;

/// One archive shown in a tab.
pub struct OpenArchive {
    pub path: PathBuf,
    pub tree: Arc<ArchiveTree>,
    pub format: String,
    pub password: Option<String>,
    /// The folder being shown, `""` for the top.
    pub cwd: String,
    /// Path of the selected row.
    pub selected: Option<String>,
    /// Result of the last integrity test: `Some(true)` passed, `Some(false)` failed.
    pub verified: Option<bool>,
}

/// Heights of the file list: its title row and each row, as drawn by the table.
const LIST_HEAD: f32 = 28.;
const LIST_ROW: f32 = 28.;

/// A drag with the mouse over the list that picks every row it reaches.
#[derive(Debug, Clone, Copy)]
pub struct Marquee {
    pub start: Point<Pixels>,
    pub current: Point<Pixels>,
}

/// The table row under a vertical position, from the top of the list and how far it has
/// scrolled. `None` over the title row; positions below the last row give an index past it.
pub fn list_row_at(y: f32, list_top: f32, scrolled: f32) -> Option<usize> {
    let rel = y - list_top - LIST_HEAD + scrolled;
    (rel >= 0.).then(|| (rel / LIST_ROW) as usize)
}

/// What the area right of the left menu shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// The files of the open archive.
    Archive,
    /// The archives opened lately.
    Recent,
    /// A folder on disk, browsed like in a file manager.
    Folder,
    Favorites,
    Passwords,
    /// What finished lately, kept across restarts.
    History,
}

/// How a row of the left menu behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// The page being shown.
    Current,
    /// Works.
    Live,
}

/// The picture shown in the details panel for the selected file.
pub struct Preview {
    pub archive: PathBuf,
    /// Path of the file inside the archive.
    pub path: String,
    pub image: Option<Arc<Image>>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Info(String),
    Error(String),
}

#[derive(Clone, PartialEq, Eq)]
pub enum PasswordTarget {
    Open(PathBuf),
    Task(TaskId),
}

pub struct PasswordPrompt {
    pub target: PasswordTarget,
    pub input: Entity<InputState>,
    pub wrong: bool,
}

/// What the new archive is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
    TarZst,
    TarXz,
    TarGz,
    TarBz2,
    TarLz4,
}

impl Format {
    pub const ALL: [Format; 7] = [
        Format::Zip,
        Format::SevenZ,
        Format::TarZst,
        Format::TarXz,
        Format::TarGz,
        Format::TarBz2,
        Format::TarLz4,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
            Format::TarZst => "tar.zst",
            Format::TarXz => "tar.xz",
            Format::TarGz => "tar.gz",
            Format::TarBz2 => "tar.bz2",
            Format::TarLz4 => "tar.lz4",
        }
    }

    pub fn supports_password(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZ)
    }

    pub fn supports_volumes(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZ)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Fast,
    Balanced,
    Smallest,
    EmailShare,
    Portable,
}

impl Preset {
    pub fn level(self) -> u32 {
        match self {
            Preset::Fast => 1,
            Preset::Balanced => 6,
            Preset::Smallest => 9,
            Preset::EmailShare | Preset::Portable => 6,
        }
    }
}

pub struct CompressForm {
    pub sources: Vec<PathBuf>,
    pub name: Entity<InputState>,
    pub password: Entity<InputState>,
    pub volume_size: Entity<InputState>,
    pub dictionary_size: Entity<InputState>,
    pub solid_block_size: Entity<InputState>,
    pub format: Format,
    pub preset: Preset,
}

pub enum Overlay {
    Transform(crate::tools::TransformForm),
    PasswordEdit(crate::tools::PasswordForm),
    PasswordDelete(String),
    RecoveryReport(TaskId),
    Password(PasswordPrompt),
    Compress(CompressForm),
    DuplicatePrompt {
        job: JobSpec,
        groups: Vec<rz_app::duplicates::DuplicateGroup>,
        scan_incomplete: bool,
    },
    Settings,
    /// Shown once, on the first start: what the anonymous statistics are.
    Privacy,
    /// Deleting these entries from the archive, waiting for the user to agree.
    ConfirmDelete(Vec<String>),
    /// A new name for an entry of the archive, or for an archive file.
    Rename(RenamePrompt),
    /// Moving an archive file to the trash, waiting for the user to agree.
    ConfirmTrash(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameTarget {
    /// A file or folder inside the open archive, by its path there.
    Entry(String),
    /// An archive file on disk.
    File(PathBuf),
}

pub struct RenamePrompt {
    pub target: RenameTarget,
    pub input: Entity<InputState>,
    pub error: Option<String>,
}

pub struct MainView {
    pub locations: crate::locations::LocationsState,
    pub(crate) directory_focus: FocusHandle,
    pub tools: crate::tools::ToolState,
    pub archives: Vec<OpenArchive>,
    pub active: usize,
    pub table: Entity<TableState<FileRows>>,
    pub search: Entity<InputState>,
    pub tasks: TaskManager,
    pub task_view: Vec<TaskInfo>,
    pub settings: Settings,
    pub(crate) settings_path: Option<PathBuf>,
    pub history: rz_app::history::History,
    pub history_path: Option<PathBuf>,
    /// Where remembered passwords live; the system keychain in the real application.
    pub vault: Arc<dyn SecretStore>,
    pub update: Option<crate::services::UpdateState>,
    pub notice: Option<Notice>,
    notice_gen: u64,
    pub overlay: Option<Overlay>,
    pub loading: Option<PathBuf>,
    pub(crate) open_requests: OpenRequests,
    pub preview: Option<Preview>,
    /// The statistics notice was opened from the settings, so it goes back there.
    pub privacy_from_settings: bool,
    pub page: Page,
    /// Where the file list was drawn last, to turn mouse positions into rows.
    files_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub marquee: Option<Marquee>,
    pub recent: RecentList,
    /// The folder shown on the folder page, and what is in it.
    pub folder: Option<PathBuf>,
    pub files: RecentList,
    /// Opens an unpacked file with the program the system uses for it. Replaced in tests.
    pub opener: Arc<dyn Fn(&Path) + Send + Sync>,
    polling: bool,
    /// Tasks already reported, so each result is announced once.
    announced: HashSet<TaskId>,
    /// Which archive each running integrity test belongs to.
    tests: HashMap<TaskId, PathBuf>,
    /// Which archive each running add/delete belongs to, to read it again afterwards.
    edits: HashMap<TaskId, PathBuf>,
    /// Compression jobs waiting for their content-based duplicate scan.
    duplicate_scans: HashMap<TaskId, JobSpec>,
}

/// Reads the whole entry list of an archive. Runs off the UI thread.
pub fn load_archive(path: &Path, password: Option<&str>) -> Result<(ArchiveTree, String), Error> {
    let mut reader = rz_formats::open(path)?;
    if let Some(pw) = password {
        reader.set_password(pw)?;
    }
    let entries = reader.entries()?;
    Ok((
        ArchiveTree::build(entries),
        reader.format_name().to_string(),
    ))
}

/// `128402` as `128,402`.
fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn duplicate_short_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "?".into());
    path.parent()
        .and_then(Path::file_name)
        .map(|parent| format!("{}/{}", parent.to_string_lossy(), name))
        .unwrap_or(name)
}

fn format_speed(bps: u64) -> String {
    format!("{}/s", format_size(bps))
}

fn format_eta(secs: u64, language: &str) -> String {
    match secs {
        0..=59 => crate::text::tr(language, "eta_seconds").replace("{seconds}", &secs.to_string()),
        60..=3599 => crate::text::tr(language, "eta_minutes_seconds")
            .replace("{minutes}", &(secs / 60).to_string())
            .replace("{seconds}", &(secs % 60).to_string()),
        _ => crate::text::tr(language, "eta_hours_minutes")
            .replace("{hours}", &(secs / 3600).to_string())
            .replace("{minutes}", &(secs % 3600 / 60).to_string()),
    }
}

fn deliver_task_notification(message: &str) {
    #[cfg(target_os = "macos")]
    {
        static READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*READY.get_or_init(|| notify_rust::set_application("com.yuhuotech.rustzip").is_ok()) {
            return;
        }
    }
    let _ = notify_rust::Notification::new()
        .summary("RustZIP")
        .body(message)
        .show();
}

pub(crate) fn kind_label(kind: JobKind, language: &str) -> &'static str {
    match kind {
        JobKind::Compress => crate::text::tr(language, "kind_compress"),
        JobKind::Extract => crate::text::tr(language, "kind_extract"),
        JobKind::Test => crate::text::tr(language, "kind_test"),
        JobKind::Edit => crate::text::tr(language, "kind_edit"),
        JobKind::Convert => crate::text::tr(language, "convert"),
        JobKind::Repair => crate::text::tr(language, "repair"),
        JobKind::Hash => crate::text::tr(language, "hash"),
        JobKind::DuplicateScan => crate::text::tr(language, "duplicate_scan"),
    }
}

fn explain(language: &str, e: &Error) -> String {
    match e {
        Error::Password => crate::text::tr(language, "error_password").into(),
        Error::UnknownFormat => crate::text::tr(language, "error_unknown_format").into(),
        Error::Unsupported(detail) => {
            crate::text::tr(language, "error_unsupported").replace("{detail}", detail)
        }
        Error::Corrupt(detail) => {
            crate::text::tr(language, "error_corrupt").replace("{detail}", detail)
        }
        Error::UnsafePath(path) => {
            crate::text::tr(language, "error_unsafe_path").replace("{path}", path)
        }
        Error::NameConflict { .. } => {
            crate::text::task_error(language, &rz_app::tasks::TaskError::from(e))
        }
        Error::LimitExceeded(detail) => {
            crate::text::tr(language, "error_limit").replace("{detail}", detail)
        }
        Error::Io(error) => {
            crate::text::tr(language, "error_io").replace("{detail}", &error.to_string())
        }
        Error::Cancelled => crate::text::tr(language, "error_cancelled").into(),
    }
}

impl MainView {
    pub fn new(
        settings: Settings,
        settings_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = settings.sanitized();
        let table = cx.new(|cx| {
            // Modified clicks build a multi-selection in `FileRows`, not a new single row.
            TableState::new(FileRows::default(), window, cx).select_on_modifier_click(false)
        });
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::text::tr(&settings.language, "search_archive"))
        });
        cx.subscribe_in(&table, window, Self::on_table_event)
            .detach();
        cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.apply_view(cx);
            }
        })
        .detach();
        Self {
            locations: crate::locations::LocationsState::default(),
            directory_focus: cx.focus_handle(),
            tools: crate::tools::ToolState::default(),
            archives: Vec::new(),
            active: 0,
            table,
            search,
            tasks: TaskManager::new(2),
            task_view: Vec::new(),
            settings,
            settings_path,
            vault: Arc::new(MemoryStore::default()),
            update: None,
            notice: None,
            notice_gen: 0,
            overlay: None,
            history: rz_app::history::History::default(),
            history_path: None,
            loading: None,
            open_requests: OpenRequests::default(),
            preview: None,
            privacy_from_settings: false,
            page: Page::Archive,
            files_bounds: Rc::default(),
            marquee: None,
            recent: RecentList::default(),
            folder: None,
            files: RecentList::for_folder(),
            opener: Arc::new(crate::services::open_file_with_system),
            polling: false,
            announced: HashSet::new(),
            tests: HashMap::new(),
            edits: HashMap::new(),
            duplicate_scans: HashMap::new(),
        }
    }

    // ----- state helpers ---------------------------------------------------------------

    pub fn current(&self) -> Option<&OpenArchive> {
        self.archives.get(self.active)
    }

    fn current_mut(&mut self) -> Option<&mut OpenArchive> {
        self.archives.get_mut(self.active)
    }

    pub fn selected_row(&self) -> Option<Row> {
        let a = self.current()?;
        a.tree.find(a.selected.as_deref()?)
    }

    fn search_text(&self, cx: &App) -> String {
        self.search.read(cx).value().to_string()
    }

    pub(crate) fn save_settings(&self, cx: &mut Context<Self>) {
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        let settings = self.settings.clone();
        let revision = self.tools.settings_revision.clone();
        let current = revision.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // Several setting changes can arrive before a previous disk write finishes.
                    static SAVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
                    let _guard = SAVE
                        .lock()
                        .map_err(|_| Error::Io(std::io::Error::other("settings busy")))?;
                    if revision.load(std::sync::atomic::Ordering::SeqCst) != current {
                        return Ok(());
                    }
                    settings.save(&path)
                })
                .await;
            if result.is_err() {
                let _ = this.update(cx, |this, cx| {
                    this.say(Notice::Error(this.tool_text("settings_error").into()), cx)
                });
            }
        })
        .detach();
    }

    /// Shows a toast for a few seconds. Errors stay a little longer.
    pub(crate) fn say(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice_gen += 1;
        let gen_now = self.notice_gen;
        let secs = match notice {
            Notice::Info(_) => 4,
            Notice::Error(_) => 5,
        };
        self.notice = Some(notice);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(secs))
                .await;
            let _ = this.update(cx, |this, cx| {
                // A newer message, or the user moving on, has already replaced this one.
                if this.notice_gen == gen_now && this.notice.is_some() {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// The user moved on to something else: what the last action reported no longer
    /// belongs in the status bar, which goes back to describing what is on screen.
    fn leave_notice(&mut self) {
        self.notice_gen += 1;
        self.notice = None;
    }

    /// Pushes the active archive, folder and search text into the list.
    pub fn apply_view(&mut self, cx: &mut Context<Self>) {
        let filter = self.search_text(cx);
        let (tree, cwd) = match self.current() {
            Some(a) => (Some(a.tree.clone()), a.cwd.clone()),
            None => (None, String::new()),
        };
        let editable = self.can_edit();
        let stem = self
            .current()
            .map(|a| recent::archive_stem_for_language(&a.path, &self.settings.language))
            .unwrap_or_default();
        let format = self.current().map(|a| a.format.clone()).unwrap_or_default();
        let language = self.settings.language.clone();
        self.table.update(cx, |t, cx| {
            t.delegate_mut()
                .set_menu_context(editable, &stem, &format, &language);
            t.delegate_mut().show(tree, &cwd, &filter);
            t.refresh(cx);
        });
        cx.notify();
    }

    // ----- opening ---------------------------------------------------------------------

    fn supersede_active_open(&mut self) {
        self.open_requests.supersede_active();
        self.loading = None;
    }

    /// Opens an archive without blocking the window.
    pub fn open_path(
        &mut self,
        path: PathBuf,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.archives.iter().position(|a| a.path == path) {
            self.supersede_active_open();
            self.settings.remember(&path);
            self.save_settings(cx);
            self.active = ix;
            self.page = Page::Archive;
            self.apply_view(cx);
            return;
        }
        let (request_id, should_start) = self.begin_open_request(&path);
        if !should_start {
            cx.notify();
            return;
        }
        self.notice = None;
        cx.notify();
        let (p, pw) = (path.clone(), password.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_archive(&p, pw.as_deref()) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_open(request_id, path, password, result, window, cx);
            });
        })
        .detach();
    }

    /// Reserves an open request so results from older, slower requests can be handled in the
    /// background without taking focus away from the archive the user opened most recently.
    pub fn begin_open_request(&mut self, path: &Path) -> (u64, bool) {
        let request = self.open_requests.begin(path);
        self.loading = Some(path.to_path_buf());
        self.notice = None;
        request
    }

    pub fn finish_open(
        &mut self,
        request_id: u64,
        path: PathBuf,
        password: Option<String>,
        result: Result<(ArchiveTree, String), Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(is_active) = self.open_requests.complete(&path, request_id) else {
            return;
        };
        match result {
            Ok((tree, format)) => {
                self.settings.remember(&path);
                self.save_settings(cx);
                if self.settings.remember_passwords
                    && let Some(pw) = &password
                {
                    self.remember_password(&path, pw, cx);
                }
                let index = if let Some(ix) = self.archives.iter().position(|a| a.path == path) {
                    ix
                } else {
                    self.archives.push(OpenArchive {
                        path,
                        tree: Arc::new(tree),
                        format,
                        password,
                        cwd: String::new(),
                        selected: None,
                        verified: None,
                    });
                    self.archives.len() - 1
                };
                if is_active {
                    self.open_requests.finish_active(request_id);
                    self.loading = None;
                    self.active = index;
                    self.page = Page::Archive;
                    self.overlay = None;
                    self.apply_view(cx);
                } else {
                    cx.notify();
                }
            }
            Err(Error::Password) => {
                if !is_active {
                    return;
                }
                // A password saved earlier gets one try before the user is asked.
                if password.is_none() && self.settings.remember_passwords {
                    self.open_with_saved_password(path, request_id, window, cx);
                    return;
                }
                self.open_requests.finish_active(request_id);
                self.loading = None;
                let wrong = password.is_some();
                self.ask_password(PasswordTarget::Open(path), wrong, window, cx);
            }
            Err(e) => {
                if !is_active {
                    return;
                }
                self.open_requests.finish_active(request_id);
                self.loading = None;
                let msg = crate::text::open_error(&self.settings.language, &path, &e);
                self.say(Notice::Error(msg.into()), cx);
            }
        }
    }

    fn render_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (message, dot) = match self.notice.as_ref()? {
            Notice::Info(m) => (m.clone(), palette(cx).accent),
            Notice::Error(m) => (m.clone(), palette(cx).danger),
        };
        let p = palette(cx);
        Some(
            div()
                .absolute()
                .top(px(TITLEBAR_HEIGHT + 12.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    h_flex()
                        .id("toast")
                        .max_w(px(420.))
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded(px(12.))
                        .bg(p.panel)
                        .border_1()
                        .border_color(p.line)
                        .shadow_md()
                        .cursor_pointer()
                        .text_color(p.fg)
                        .child(div().size_2().flex_none().rounded_full().bg(dot))
                        .child(message)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.leave_notice();
                            cx.notify();
                        })),
                ),
        )
    }

    pub fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.archives.len() {
            self.archives.remove(ix);
            self.active = self.active.min(self.archives.len().saturating_sub(1));
            if self.archives.is_empty() {
                self.page = Page::Archive;
            }
            self.leave_notice();
            self.apply_view(cx);
        }
    }

    pub fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.archives.len() {
            self.supersede_active_open();
            self.active = ix;
            self.page = Page::Archive;
            self.leave_notice();
            self.apply_view(cx);
        }
    }

    pub fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.supersede_active_open();
            self.leave_notice();
            // What was on screen has been seen, so the menu badges clear on the way out.
            if self.page == Page::History && self.history.mark_all_seen() {
                self.save_history(cx);
            }
        }
        self.page = page;
        if page == Page::Recent {
            self.refresh_recent(cx);
        }
        if page == Page::Passwords {
            self.refresh_password_book(cx);
        }
        if page == Page::Favorites {
            self.refresh_locations(cx);
        }
        cx.notify();
    }

    pub fn pick_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(self.tool_text("dialog_open_archive").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    for p in paths {
                        this.open_path(p, None, window, cx);
                    }
                });
            }
        })
        .detach();
    }

    pub fn pick_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.tool_text("dialog_choose_sources").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_compress(paths, window, cx);
                });
            }
        })
        .detach();
    }

    /// Files dropped on the window: archives are opened, anything else is offered for compression.
    pub fn handle_drop(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (archives, others): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| {
            p.is_file()
                && rz_formats::open(p)
                    .map(|_| true)
                    .unwrap_or_else(|e| matches!(e, Error::Password))
        });
        for a in archives {
            self.open_path(a, None, window, cx);
        }
        if !others.is_empty() {
            self.open_compress(others, window, cx);
        }
    }

    // ----- browsing --------------------------------------------------------------------

    pub fn navigate(&mut self, dir: &str, cx: &mut Context<Self>) {
        if let Some(a) = self.current_mut()
            && a.tree.is_dir(dir)
        {
            a.cwd = rz_core::clean_path(dir);
            a.selected = None;
        }
        self.leave_notice();
        self.preview = None;
        self.apply_view(cx);
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.current() {
            let parent = rz_app::browse::parent_dir(&a.cwd);
            self.navigate(&parent, cx);
        }
    }

    /// "Back": one folder up inside the archive; at the top of the archive, out of it to
    /// the folder the archive lies in, so browsing can go on from there.
    pub fn go_up_or_out(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        if a.cwd.is_empty() {
            if let Some(dir) = a.path.parent().map(Path::to_path_buf) {
                self.show_folder(dir, cx);
            }
        } else {
            self.go_up(cx);
        }
    }

    /// Nothing is picked, but the table keeps its own highlight (for the "back" row).
    fn pick_nothing(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.current_mut() {
            a.selected = None;
        }
        self.preview = None;
        self.table.update(cx, |t, _| t.delegate_mut().clear_marks());
        self.leave_notice();
        cx.notify();
    }

    // ----- browsing folders on disk ------------------------------------------------------

    /// Shows the files of a folder on disk, like a file manager, so the archive manager
    /// can be used to move around and open the next archive from.
    pub fn show_folder(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.supersede_active_open();
        self.leave_notice();
        self.page = Page::Folder;
        self.reset_directory_scroll();
        self.folder = Some(dir.clone());
        self.files.selected = None;
        self.files.items.clear();
        self.reload_folder(cx);
        cx.notify();
    }

    /// Reads the folder being browsed again, off the UI thread.
    pub fn reload_folder(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.folder.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let target = dir.clone();
            let result = cx
                .background_executor()
                .spawn(async move { recent::list_dir(&target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                // The user may have moved on to another folder while this was read.
                if this.folder.as_ref() != Some(&dir) {
                    return;
                }
                match result {
                    Ok(items) => this.files.set_items(items),
                    Err(e) => {
                        this.files.set_items(Vec::new());
                        this.say(
                            Notice::Error(
                                this.tool_text("notice_folder_open_failed")
                                    .replace("{error}", &e.to_string()),
                            ),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One folder up on disk.
    pub fn folder_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self
            .folder
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
        {
            self.show_folder(parent, cx);
        }
    }

    /// The list of the page that is showing one (recent archives or a folder).
    fn listed(&self) -> &RecentList {
        match self.page {
            Page::Favorites => &self.locations.list,
            Page::Folder => &self.files,
            _ => &self.recent,
        }
    }

    fn listed_mut(&mut self) -> &mut RecentList {
        match self.page {
            Page::Favorites => &mut self.locations.list,
            Page::Folder => &mut self.files,
            _ => &mut self.recent,
        }
    }

    /// Reads the shown list again after something on disk changed.
    fn refresh_listed(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Page::Folder => self.reload_folder(cx),
            Page::Recent => self.refresh_recent(cx),
            Page::Passwords => self.refresh_password_book(cx),
            Page::Favorites => self.refresh_locations(cx),
            Page::Archive | Page::History => {}
        }
    }

    /// A double click in a list of files: folders are entered, archives opened, anything
    /// else is handed to the program the system uses for it.
    pub fn open_listed(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if is_dir {
            self.show_folder(path, cx);
        } else if self.page == Page::Recent
            || recent::looks_like_archive(
                &path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        {
            self.open_path(path, None, window, cx);
        } else if crate::services::is_runnable(&path.to_string_lossy()) {
            self.say(Notice::Info(self.tool_text("notice_safe_run").into()), cx);
        } else {
            (self.opener)(&path);
        }
    }

    fn on_table_event(
        &mut self,
        table: &Entity<TableState<FileRows>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row_at = |ix: usize, cx: &App| table.read(cx).delegate().row(ix).cloned();
        match event {
            // A right click on the empty part of the list: nothing stays picked.
            TableEvent::ClearSelection => self.clear_pick(cx),
            // Listed before the general case below, which would find no row at 0.
            TableEvent::SelectRow(0) => self.pick_nothing(cx),
            TableEvent::SelectRow(ix) => {
                if let Some(row) = row_at(*ix, cx) {
                    let path = row.path;
                    self.table
                        .update(cx, |t, _| t.delegate_mut().mark_only(&path));
                    if let Some(a) = self.current_mut() {
                        a.selected = Some(path);
                    }
                    self.leave_notice();
                    self.load_preview(cx);
                    cx.notify();
                }
            }
            // The "back" row on top: one level up, or out of the archive at the top.
            TableEvent::DoubleClickedRow(0) => self.go_up_or_out(cx),
            TableEvent::DoubleClickedRow(ix) => {
                if let Some(row) = row_at(*ix, cx) {
                    match row.kind {
                        RowKind::Dir => {
                            // Leaving a search by opening a folder also clears the search.
                            self.search.update(cx, |s, cx| s.set_value("", window, cx));
                            self.navigate(&row.path, cx);
                        }
                        // Like in a folder: a double click opens the file with its program.
                        RowKind::File => {
                            if let Some(a) = self.current_mut() {
                                a.selected = Some(row.path);
                            }
                            self.open_selected(cx);
                        }
                        RowKind::Link => {}
                    }
                }
            }
            _ => {}
        }
    }

    // ----- tasks -----------------------------------------------------------------------

    fn extract_options(&self, only: Option<Vec<String>>) -> rz_engine::ExtractOptions {
        let password = self.current().and_then(|a| a.password.clone());
        let mut opts = self.settings.extract_options(password);
        opts.only = only;
        opts
    }

    /// Asks where to extract, then queues the job.
    pub fn extract(&mut self, selected_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        let only = if selected_only {
            let marked = self.table.read(cx).delegate().marked();
            match (&a.selected, marked.is_empty()) {
                (_, false) => Some(marked),
                (Some(p), true) => Some(vec![p.clone()]),
                (None, true) => {
                    self.say(
                        Notice::Info(self.tool_text("notice_select_item").into()),
                        cx,
                    );
                    return;
                }
            }
        } else {
            None
        };
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.tool_text("dialog_extract_to").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(dest) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.start_extract(dest, only, cx));
            }
        })
        .detach();
    }

    /// "Extract here": next to the archive, no questions asked.
    pub fn extract_here(&mut self, archive: PathBuf, cx: &mut Context<Self>) {
        let dest = archive
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let spec = JobSpec::Extract {
            archive,
            dest,
            opts: self.settings.extract_options(None),
            smart: self.settings.smart_extract,
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        self.submit(spec, cx);
    }

    pub fn start_extract(
        &mut self,
        dest: PathBuf,
        only: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(a) = self.current() else { return };
        let smart = self.settings.smart_extract && only.is_none();
        let spec = JobSpec::Extract {
            archive: a.path.clone(),
            dest,
            opts: self.extract_options(only),
            smart,
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        self.submit(spec, cx);
    }

    pub fn start_test(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        let spec = JobSpec::Test {
            archive: a.path.clone(),
            password: a.password.clone(),
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        let archive = a.path.clone();
        let id = self.submit(spec, cx);
        self.tests.insert(id, archive);
    }

    // ----- changing and previewing --------------------------------------------------

    /// What the user has picked in the list: the marked rows, else the selected one.
    pub fn picked_paths(&self, cx: &App) -> Vec<String> {
        let marked = self.table.read(cx).delegate().marked();
        match (self.current().and_then(|a| a.selected.clone()), marked) {
            (_, m) if !m.is_empty() => m,
            (Some(p), _) => vec![p],
            _ => Vec::new(),
        }
    }

    /// Whether the open archive is of a kind that can be changed.
    pub fn can_edit(&self) -> bool {
        self.current()
            .is_some_and(|a| rz_app::modify::edit_format(&a.format).is_some())
    }

    /// "Extract to…": the picked items when there are any, otherwise everything.
    pub fn extract_to(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let any = !self.picked_paths(cx).is_empty();
        self.extract(any, window, cx);
    }

    // ----- the right-click menu of the list ------------------------------------------------

    // ----- picking with a rubber band ------------------------------------------------------

    /// Where the file list was drawn last (for tests that move the mouse over it).
    pub fn list_bounds(&self) -> Bounds<Pixels> {
        self.files_bounds.get()
    }

    /// The row index and the scrolled distance for a mouse position over the list.
    fn list_hit(&self, y: Pixels, cx: &App) -> (Option<usize>, usize) {
        let top = self.files_bounds.get().origin.y;
        let scrolled = -self
            .table
            .read(cx)
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .offset()
            .y;
        let len = self.table.read(cx).delegate().table_len();
        (
            list_row_at(f32::from(y), f32::from(top), f32::from(scrolled)),
            len,
        )
    }

    /// A press on the empty part of the list starts a rubber band and lets go of what was
    /// picked, like in a file manager. A press on a row is the table's own business.
    pub fn start_marquee(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.page != Page::Archive || self.current().is_none() {
            return;
        }
        let (row, len) = self.list_hit(at.y, cx);
        if row.is_some_and(|r| r >= len) {
            self.marquee = Some(Marquee {
                start: at,
                current: at,
            });
            self.clear_pick(cx);
        }
    }

    /// While the button is down, everything the band touches is picked.
    pub fn drag_marquee(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(mut band) = self.marquee else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.end_marquee(cx);
            return;
        }
        band.current = ev.position;
        self.marquee = Some(band);
        let (top_y, bottom_y) = (
            band.start.y.min(band.current.y),
            band.start.y.max(band.current.y),
        );
        let ((first, len), (last, _)) = (self.list_hit(top_y, cx), self.list_hit(bottom_y, cx));
        self.table.update(cx, |t, cx| {
            match (first, last) {
                // The band reaches the rows: from where it starts to where it ends.
                (_, Some(last)) if len > 0 && first.is_none_or(|f| f < len) => {
                    t.delegate_mut()
                        .mark_between(first.unwrap_or(0), last.min(len - 1));
                }
                _ => t.delegate_mut().clear_marks(),
            }
            cx.notify();
        });
        cx.notify();
    }

    /// The button is up: what the band picked stays picked, and the details panel shows it.
    pub fn end_marquee(&mut self, cx: &mut Context<Self>) {
        if self.marquee.take().is_none() {
            return;
        }
        let marked = self.table.read(cx).delegate().marked();
        if let Some(a) = self.current_mut() {
            a.selected = match marked.as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            };
        }
        self.load_preview(cx);
        cx.notify();
    }

    /// Nothing is picked: not in the list, not for the details panel.
    pub fn clear_pick(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.current_mut() {
            a.selected = None;
        }
        self.preview = None;
        self.leave_notice();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().clear_marks();
            // The table keeps a row of its own highlighted; it must let go too.
            if t.selected_row().is_some() {
                t.clear_selection(cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    /// Picks every row of the folder being shown (⌘A / Ctrl+A).
    pub fn select_all_rows(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |t, cx| {
            t.delegate_mut().mark_all();
            cx.notify();
        });
        self.leave_notice();
        cx.notify();
    }

    /// Unpacks the picked items to `dest`, keeping their folders.
    fn extract_picked_to(&mut self, dest: PathBuf, cx: &mut Context<Self>) {
        let picked = self.picked_paths(cx);
        if picked.is_empty() {
            self.say(
                Notice::Info(self.tool_text("notice_select_item").into()),
                cx,
            );
            return;
        }
        self.start_extract(dest, Some(picked), cx);
    }

    /// Next to the archive: "extract here".
    pub fn extract_picked_here(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self
            .current()
            .and_then(|a| a.path.parent().map(Path::to_path_buf))
        else {
            return;
        };
        self.extract_picked_to(dir, cx);
    }

    /// Next to the archive, in a folder named after it.
    pub fn extract_picked_named(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        let Some(parent) = a.path.parent() else {
            return;
        };
        let dir = parent.join(recent::archive_stem_for_language(
            &a.path,
            &self.settings.language,
        ));
        self.extract_picked_to(dir, cx);
    }

    /// Puts the in-archive paths of the picked items on the clipboard, one per line.
    pub fn copy_picked_paths(&mut self, cx: &mut Context<Self>) {
        let picked = self.picked_paths(cx);
        if picked.is_empty() {
            return;
        }
        let n = picked.len();
        cx.write_to_clipboard(ClipboardItem::new_string(picked.join("\n")));
        self.say(
            Notice::Info(
                self.tool_text("notice_copied_paths")
                    .replace("{count}", &n.to_string()),
            ),
            cx,
        );
    }

    /// "Open" in the menu: a folder is entered, a file is opened with its program.
    pub fn open_picked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self.picked_paths(cx);
        let [path] = picked.as_slice() else { return };
        let Some(row) = self.current().and_then(|a| a.tree.find(path)) else {
            return;
        };
        match row.kind {
            RowKind::Dir => {
                self.search.update(cx, |s, cx| s.set_value("", window, cx));
                self.navigate(path, cx);
            }
            RowKind::File => {
                if let Some(a) = self.current_mut() {
                    a.selected = Some(path.clone());
                }
                self.open_selected(cx);
            }
            RowKind::Link => {}
        }
    }

    /// Adds files into the one folder that is picked.
    pub fn add_into_picked_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self.picked_paths(cx);
        let [path] = picked.as_slice() else { return };
        if self
            .current()
            .and_then(|a| a.tree.find(path))
            .map(|r| r.kind)
            != Some(RowKind::Dir)
        {
            return;
        }
        self.pick_files_to_add_into(path.clone(), window, cx);
    }

    /// Asks which files to add, then adds them to the folder being shown.
    pub fn pick_files_to_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let into = self.current().map(|a| a.cwd.clone()).unwrap_or_default();
        self.pick_files_to_add_into(into, window, cx);
    }

    fn pick_files_to_add_into(
        &mut self,
        into: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_edit() {
            self.say(Notice::Info(self.tool_text("notice_read_only").into()), cx);
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(self.tool_text("dialog_add_to_archive").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |this, cx| this.add_files_into(into, paths, cx));
            }
        })
        .detach();
    }

    /// Adds files and folders to the folder being shown. The archive is rewritten.
    pub fn add_files(&mut self, sources: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        let into = a.cwd.clone();
        self.add_files_into(into, sources, cx);
    }

    fn add_files_into(&mut self, into: String, sources: Vec<PathBuf>, cx: &mut Context<Self>) {
        if sources.is_empty() || !self.can_edit() {
            return;
        }
        self.submit_edit(rz_app::modify::Edit::Add { into, sources }, cx);
    }

    /// Opens the "are you sure" card for deleting the picked items.
    pub fn ask_delete(&mut self, cx: &mut Context<Self>) {
        let paths = self.picked_paths(cx);
        if paths.is_empty() || !self.can_edit() {
            return;
        }
        self.overlay = Some(Overlay::ConfirmDelete(paths));
        cx.notify();
    }

    pub fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        if let Some(Overlay::ConfirmDelete(paths)) = self.overlay.take() {
            self.submit_edit(rz_app::modify::Edit::Delete(paths), cx);
        }
        cx.notify();
    }

    fn submit_edit(&mut self, edit: rz_app::modify::Edit, cx: &mut Context<Self>) {
        let Some(a) = self.current() else { return };
        let archive = a.path.clone();
        self.submit_edit_on(archive, edit, cx);
    }

    /// Queues a change to an open archive, which need not be the one on screen.
    fn submit_edit_on(
        &mut self,
        archive: PathBuf,
        edit: rz_app::modify::Edit,
        cx: &mut Context<Self>,
    ) {
        let password = self
            .archives
            .iter()
            .find(|a| a.path == archive)
            .and_then(|a| a.password.clone());
        let mut opts = self.settings.extract_options(password);
        opts.only = None;
        let spec = JobSpec::Edit {
            archive: archive.clone(),
            edit,
            opts,
            level: self.settings.default_level,
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        let id = self.submit(spec, cx);
        self.edits.insert(id, archive);
    }

    // ----- renaming ---------------------------------------------------------------------

    fn open_rename(
        &mut self,
        target: RenameTarget,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(self.tool_text("rename_placeholder"))
        });
        input.update(cx, |s, cx| s.set_value(current, window, cx));
        self.overlay = Some(Overlay::Rename(RenamePrompt {
            target,
            input,
            error: None,
        }));
        cx.notify();
    }

    /// Renames the picked file or folder inside the archive.
    pub fn ask_rename_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = self.picked_paths(cx);
        let [path] = picked.as_slice() else { return };
        if !self.can_edit() {
            return;
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        self.open_rename(RenameTarget::Entry(path.clone()), &name, window, cx);
    }

    /// Renames the selected recent archive on disk.
    pub fn ask_rename_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.listed().selected_item().filter(|i| i.exists) else {
            return;
        };
        let (path, name) = (item.path.clone(), item.name.clone());
        self.open_rename(RenameTarget::File(path), &name, window, cx);
    }

    pub fn submit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Rename(mut prompt)) = self.overlay.take() else {
            return;
        };
        let name = prompt.input.read(cx).value().trim().to_string();
        if !rz_app::modify::valid_name(&name) {
            prompt.error = Some(self.tool_text("rename_invalid").into());
            self.overlay = Some(Overlay::Rename(prompt));
            cx.notify();
            return;
        }
        match prompt.target {
            RenameTarget::Entry(path) => {
                if path.rsplit('/').next() != Some(name.as_str()) {
                    self.submit_edit(
                        rz_app::modify::Edit::Rename {
                            path,
                            new_name: name,
                        },
                        cx,
                    );
                }
            }
            RenameTarget::File(from) => self.rename_file(from, name, window, cx),
        }
        cx.notify();
    }

    fn rename_file(
        &mut self,
        from: PathBuf,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(to) = from.parent().map(|d| d.join(&name)) else {
            return;
        };
        if to == from {
            return;
        }
        let (a, b) = (from.clone(), to.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // `rename` would silently replace an existing file on most systems.
                    if b.exists() {
                        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
                    }
                    std::fs::rename(&a, &b)
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    for r in &mut this.settings.recent {
                        if *r == from {
                            *r = to.clone();
                        }
                    }
                    for a in &mut this.archives {
                        if a.path == from {
                            a.path = to.clone();
                        }
                    }
                    this.save_settings(cx);
                    this.listed_mut().selected = Some(to);
                    this.refresh_listed(cx);
                    this.say(Notice::Info(this.tool_text("rename_complete").into()), cx);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    this.say(Notice::Error(this.tool_text("rename_exists").into()), cx)
                }
                Err(e) => this.say(
                    Notice::Error(
                        this.tool_text("rename_failed")
                            .replace("{error}", &e.to_string()),
                    ),
                    cx,
                ),
            });
        })
        .detach();
    }

    // ----- recent archives --------------------------------------------------------------

    /// Reads the sizes and dates of the recent files in the background.
    pub fn refresh_recent(&mut self, cx: &mut Context<Self>) {
        let paths = self.settings.recent.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { recent::scan(&paths) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.recent.set_items(items);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn select_recent(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.listed_mut().selected = Some(path);
        self.leave_notice();
        cx.notify();
    }

    pub fn sort_recent(&mut self, key: RecentSort, cx: &mut Context<Self>) {
        self.listed_mut().sort_by(key);
        cx.notify();
    }

    /// Takes a file off the list; the file itself stays where it is.
    pub fn forget_recent(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.settings.recent.retain(|p| p != path);
        self.save_settings(cx);
        self.recent.items.retain(|i| i.path != path);
        self.recent.selected = None;
        cx.notify();
    }

    pub fn clear_recent(&mut self, cx: &mut Context<Self>) {
        self.settings.recent.clear();
        self.save_settings(cx);
        self.recent.set_items(Vec::new());
        self.recent.selected = None;
        cx.notify();
    }

    /// Unpacks the selected recent archive next to it, optionally into a folder named
    /// after it.
    pub fn extract_recent(&mut self, into_named_folder: bool, cx: &mut Context<Self>) {
        let Some(item) = self.listed().selected_item().filter(|i| i.exists) else {
            return;
        };
        let archive = item.path.clone();
        let Some(parent) = archive.parent() else {
            return;
        };
        let dest = if into_named_folder {
            parent.join(recent::archive_stem_for_language(
                &archive,
                &self.settings.language,
            ))
        } else {
            parent.to_path_buf()
        };
        let spec = JobSpec::Extract {
            archive,
            dest,
            opts: self.settings.extract_options(None),
            smart: false,
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        self.submit(spec, cx);
    }

    pub fn ask_trash(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = self.listed().selected_item().filter(|i| i.exists) {
            self.overlay = Some(Overlay::ConfirmTrash(item.path.clone()));
            cx.notify();
        }
    }

    /// Moves the archive to the system trash, so it can still be brought back.
    pub fn confirm_trash(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::ConfirmTrash(path)) = self.overlay.take() else {
            return;
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { rz_services::move_to_trash(&target) })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    // It is gone from disk, so it must not stay open or listed.
                    if let Some(ix) = this.archives.iter().position(|a| a.path == path) {
                        this.close_tab(ix, cx);
                    }
                    this.forget_recent(&path, cx);
                    this.files.selected = None;
                    this.refresh_listed(cx);
                    this.say(Notice::Info(this.tool_text("trash_complete").into()), cx);
                }
                Err(e) => this.say(
                    Notice::Error(
                        this.tool_text("trash_failed")
                            .replace("{error}", &e.to_string()),
                    ),
                    cx,
                ),
            });
        })
        .detach();
    }

    // ----- opening a file from inside an archive ------------------------------------------

    /// Watches a file that was unpacked for viewing. When the program that opened it saves
    /// a change, the new content goes back into the archive.
    ///
    /// The check runs every moment and only acts once the file has stopped changing, so a
    /// program that saves in several steps does not trigger several rewrites.
    fn watch_scratch(
        &mut self,
        scratch: PathBuf,
        archive: PathBuf,
        entry: String,
        cx: &mut Context<Self>,
    ) {
        fn stamp(path: &Path) -> Option<(Option<std::time::SystemTime>, u64)> {
            let m = std::fs::metadata(path).ok()?;
            Some((m.modified().ok(), m.len()))
        }
        cx.spawn(async move |this, cx| {
            let first = {
                let p = scratch.clone();
                cx.background_executor()
                    .spawn(async move { stamp(&p) })
                    .await
            };
            let (mut saved, mut waiting) = (first, None);
            let mut missing = 0u32;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1200))
                    .await;
                let p = scratch.clone();
                let now = cx
                    .background_executor()
                    .spawn(async move { stamp(&p) })
                    .await;
                // Closing the archive, or the scratch file going away, ends the watch.
                let open = this
                    .update(cx, |this, _| {
                        this.archives.iter().any(|a| a.path == archive)
                    })
                    .unwrap_or(false);
                if !open {
                    break;
                }
                let Some(now) = now else {
                    // Some programs replace the file by deleting and recreating it.
                    missing += 1;
                    if missing > 50 {
                        break;
                    }
                    continue;
                };
                missing = 0;
                if Some(now) == saved {
                    waiting = None;
                    continue;
                }
                if waiting != Some(now) {
                    waiting = Some(now);
                    continue;
                }
                // Changed, and unchanged since the last look: save it back.
                let (scratch2, archive2, entry2) =
                    (scratch.clone(), archive.clone(), entry.clone());
                let sent = this
                    .update(cx, |this, cx| {
                        // One rewrite at a time per archive; try again on the next look.
                        if this.edits.values().any(|p| *p == archive2) {
                            return false;
                        }
                        this.submit_edit_on(
                            archive2,
                            rz_app::modify::Edit::Replace {
                                entry: entry2,
                                with: scratch2,
                            },
                            cx,
                        );
                        true
                    })
                    .unwrap_or(false);
                if sent {
                    saved = Some(now);
                    waiting = None;
                }
            }
        })
        .detach();
    }

    /// Reads the archive again after it was changed, keeping the folder being shown.
    fn reload_archive(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let password = self
            .archives
            .iter()
            .find(|a| a.path == path)
            .and_then(|a| a.password.clone());
        cx.spawn(async move |this, cx| {
            let (p, pw) = (path.clone(), password);
            let result = cx
                .background_executor()
                .spawn(async move { load_archive(&p, pw.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((tree, _)) => {
                        if let Some(a) = this.archives.iter_mut().find(|a| a.path == path) {
                            a.tree = Arc::new(tree);
                            // The folder being shown may have been deleted.
                            while !a.cwd.is_empty() && !a.tree.is_dir(&a.cwd) {
                                a.cwd = rz_app::browse::parent_dir(&a.cwd);
                            }
                            // Keep the pick when the item is still there, so saving a file
                            // from its editor does not lose the user's place.
                            if a.selected
                                .as_deref()
                                .is_some_and(|s| a.tree.find(s).is_none())
                            {
                                a.selected = None;
                            }
                            a.verified = None;
                        }
                        this.preview = None;
                        this.apply_view(cx);
                    }
                    Err(e) => this.say(Notice::Error(explain(&this.settings.language, &e)), cx),
                };
            });
        })
        .detach();
    }

    /// Unpacks the picked file to a scratch folder and opens it with the program the
    /// system uses for that kind of file.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            self.say(
                Notice::Info(self.tool_text("notice_select_file").into()),
                cx,
            );
            return;
        };
        let Some(a) = self.current() else { return };
        let entry = row.entry.and_then(|ix| a.tree.entries().get(ix).cloned());
        let (RowKind::File, Some(entry)) = (row.kind, entry) else {
            self.say(
                Notice::Info(self.tool_text("notice_preview_file").into()),
                cx,
            );
            return;
        };
        // Opening a program or script from an archive would run it, which is not a preview.
        if crate::services::is_runnable(&row.name) {
            self.say(Notice::Info(self.tool_text("notice_safe_run").into()), cx);
            return;
        }
        let (archive, password) = (a.path.clone(), a.password.clone());
        let encoding = self.settings.forced_encoding().map(str::to_string);
        let name = crate::services::scratch_name(&row.name);
        // Formats that can be rewritten get their changes saved back; the rest are read-only.
        let writable = rz_app::modify::edit_format(&a.format).is_some();
        let (entry_path, archive_key) = (row.path.clone(), archive.clone());
        self.say(
            Notice::Info(
                self.tool_text("notice_prepare_preview")
                    .replace("{name}", &row.name),
            ),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let data = rz_app::preview::read_entry(
                        &archive,
                        &entry,
                        password.as_deref(),
                        encoding.as_deref(),
                        crate::services::MAX_PREVIEW_BYTES,
                    )?;
                    crate::services::write_scratch(&name, &data).map_err(Error::Io)
                })
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => {
                    (this.opener)(&path);
                    if writable {
                        this.say(
                            Notice::Info(this.tool_text("notice_external_edit_writeback").into()),
                            cx,
                        );
                        this.watch_scratch(path, archive_key, entry_path, cx);
                    } else {
                        this.say(
                            Notice::Info(this.tool_text("notice_external_read_only").into()),
                            cx,
                        );
                    }
                }
                Err(Error::LimitExceeded(_)) => this.say(
                    Notice::Info(this.tool_text("notice_preview_too_large").into()),
                    cx,
                ),
                Err(e) => this.say(Notice::Error(explain(&this.settings.language, &e)), cx),
            });
        })
        .detach();
    }

    fn submit(&mut self, spec: JobSpec, cx: &mut Context<Self>) -> TaskId {
        let id = self.tasks.submit(spec);
        self.notice = None;
        // Callers may need to attach a test/edit record before a fast task is observed as done.
        self.task_view = self.tasks.snapshot();
        self.ensure_polling(cx);
        cx.notify();
        id
    }

    /// Starts reading a bounded picture or text file for the details panel.
    ///
    /// Only small pictures and bounded UTF-8 text are read; other files keep the type tile.
    pub fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            self.preview = None;
            return;
        };
        let Some(a) = self.current() else { return };
        let image_format = rz_app::preview::image_format(&row.name);
        let text_format = rz_app::preview::is_text_format(&row.name);
        let entry = row.entry.and_then(|ix| a.tree.entries().get(ix).cloned());
        let (Some(entry), RowKind::File) = (entry, row.kind) else {
            self.preview = None;
            return;
        };
        let limit = if image_format.is_some() {
            rz_app::preview::MAX_IMAGE_BYTES
        } else if text_format {
            rz_app::preview::MAX_TEXT_BYTES
        } else {
            self.preview = None;
            return;
        };
        let (archive, password) = (a.path.clone(), a.password.clone());
        if self
            .preview
            .as_ref()
            .is_some_and(|p| p.archive == archive && p.path == row.path)
        {
            return;
        }
        self.preview = None;
        if row.size > limit {
            return;
        }
        let encoding = self.settings.forced_encoding().map(str::to_string);
        let path = row.path;
        cx.spawn(async move |this, cx| {
            let (archive_bg, encoding_bg) = (archive.clone(), encoding);
            let data = cx
                .background_executor()
                .spawn(async move {
                    rz_app::preview::read_entry(
                        &archive_bg,
                        &entry,
                        password.as_deref(),
                        encoding_bg.as_deref(),
                        limit,
                    )
                })
                .await;
            // A picture that cannot be read just keeps the type tile.
            let Ok(data) = data else { return };
            let (image, text) = if let Some(format) = image_format {
                let format = match format {
                    rz_app::preview::ImageFormat::Png => ImageFormat::Png,
                    rz_app::preview::ImageFormat::Jpeg => ImageFormat::Jpeg,
                    rz_app::preview::ImageFormat::Gif => ImageFormat::Gif,
                    rz_app::preview::ImageFormat::Webp => ImageFormat::Webp,
                    rz_app::preview::ImageFormat::Bmp => ImageFormat::Bmp,
                };
                (Some(Arc::new(Image::from_bytes(format, data))), None)
            } else {
                let Some(text) = rz_app::preview::decode_text(data) else {
                    return;
                };
                (None, Some(text))
            };
            let _ = this.update(cx, |this, cx| {
                // The selection may have moved on while the file was being read.
                let still_here = this.current().is_some_and(|a| {
                    a.path == archive && a.selected.as_deref() == Some(path.as_str())
                });
                if still_here {
                    this.preview = Some(Preview {
                        archive,
                        path,
                        image,
                        text,
                    });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Re-reads the task list and announces tasks that just ended.
    pub fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        self.task_view = self.tasks.snapshot();
        for t in self.task_view.clone() {
            if !t.state.is_finished() || !self.announced.insert(t.id) {
                continue;
            }
            // A task that stopped for a missing password is retried as a new task, which
            // takes over these records, so they must survive until then.
            let needs_password = t.state == TaskState::Failed
                && t.error
                    .as_ref()
                    .is_some_and(|e| e.code == ErrorCode::Password);
            if !needs_password {
                if let Some(path) = self.tests.remove(&t.id) {
                    let verdict = match t.state {
                        TaskState::Done => Some(true),
                        TaskState::Failed => Some(false),
                        _ => None,
                    };
                    if let Some(a) = self.archives.iter_mut().find(|a| a.path == path) {
                        a.verified = verdict;
                    }
                }
                if let Some(path) = self.edits.remove(&t.id)
                    && t.state == TaskState::Done
                {
                    self.reload_archive(path, cx);
                }
            }
            match (&t.state, &t.error, &t.summary) {
                (TaskState::Done, _, Some(s)) => {
                    let what = kind_label(t.kind, &self.settings.language);
                    let msg = match t.kind {
                        JobKind::Convert => self.tool_text("converted").into(),
                        JobKind::Repair => self.tool_text("recovered").into(),
                        JobKind::Test => self
                            .tool_text("task_test_passed")
                            .replace("{task}", what)
                            .replace("{count}", &s.files.to_string()),
                        JobKind::Edit => self
                            .tool_text("task_edit_complete")
                            .replace("{task}", what)
                            .replace("{count}", &s.files.to_string()),
                        JobKind::Hash => self.tool_text("hash_complete").to_string(),
                        JobKind::DuplicateScan if s.duplicates.is_empty() => {
                            self.tool_text("duplicate_none").to_string()
                        }
                        JobKind::DuplicateScan => {
                            let files: usize = s
                                .duplicates
                                .iter()
                                .map(rz_app::duplicates::DuplicateGroup::duplicate_count)
                                .sum();
                            let bytes = s.duplicates.iter().fold(0u64, |total, group| {
                                total.saturating_add(
                                    group.size.saturating_mul(group.duplicate_count() as u64),
                                )
                            });
                            self.tool_text("duplicate_found")
                                .replace("{groups}", &s.duplicates.len().to_string())
                                .replace("{files}", &files.to_string())
                                .replace("{size}", &format_size(bytes))
                        }
                        _ => self
                            .tool_text("task_complete_count")
                            .replace("{task}", what)
                            .replace("{count}", &s.files.to_string())
                            .replace("{size}", &format_size(s.bytes)),
                    };
                    if t.kind != JobKind::DuplicateScan {
                        self.record_task(&t, &msg, cx);
                    }
                    let toast = if t.kind == JobKind::Hash {
                        let values = s
                            .hashes
                            .iter()
                            .map(|digest| format!("{}  {}", digest.algorithm.name(), digest.hex))
                            .collect::<Vec<_>>()
                            .join("\n");
                        format!("{msg}\n{values}")
                    } else {
                        msg
                    };
                    if t.kind != JobKind::DuplicateScan || !s.duplicates.is_empty() {
                        self.say(Notice::Info(toast), cx);
                    }
                    if t.kind != JobKind::DuplicateScan {
                        self.send_task_notification("notification_done", cx);
                    }
                    if self.settings.open_folder_when_done && t.kind == JobKind::Extract {
                        cx.reveal_path(&s.reveal);
                    }
                    let pending_compression = if t.kind == JobKind::DuplicateScan {
                        self.duplicate_scans.remove(&t.id)
                    } else {
                        None
                    };
                    if let Some(job) = pending_compression {
                        if s.duplicates.is_empty() {
                            self.submit(job, cx);
                        } else {
                            self.overlay = Some(Overlay::DuplicatePrompt {
                                job,
                                groups: s.duplicates.clone(),
                                scan_incomplete: false,
                            });
                        }
                    }
                }
                // Retried as a new task once the password is in, so there is nothing to record.
                (TaskState::Failed, Some(err), _) if err.code == ErrorCode::Password => {
                    self.say(
                        Notice::Error(
                            self.tool_text("task_password_needed")
                                .replace("{task}", &t.title),
                        ),
                        cx,
                    );
                }
                (TaskState::Failed, Some(err), _)
                    if t.kind == JobKind::DuplicateScan && err.code == ErrorCode::LimitExceeded =>
                {
                    if let Some(job) = self.duplicate_scans.remove(&t.id) {
                        self.overlay = Some(Overlay::DuplicatePrompt {
                            job,
                            groups: Vec::new(),
                            scan_incomplete: true,
                        });
                        self.say(
                            Notice::Info(self.tool_text("duplicate_scan_limit").into()),
                            cx,
                        );
                        continue;
                    }
                }
                (TaskState::Failed, Some(err), _) => {
                    let why = crate::text::task_error(&self.settings.language, err);
                    self.record_task(&t, &why, cx);
                    self.say(
                        Notice::Error(
                            self.tool_text("task_failed_detail")
                                .replace("{task}", &t.title)
                                .replace("{reason}", &why),
                        ),
                        cx,
                    );
                    self.send_task_notification("notification_failed", cx);
                }
                (TaskState::Cancelled, _, _) => {
                    self.duplicate_scans.remove(&t.id);
                    self.record_task(&t, "", cx);
                }
                _ => {}
            }
        }
        cx.notify();
    }

    fn send_task_notification(&self, message_key: &str, cx: &mut Context<Self>) {
        if !self.settings.task_notifications {
            return;
        }
        let message = self.tool_text(message_key).to_string();
        cx.spawn(async move |_, cx| {
            let _ = cx
                .background_executor()
                .spawn(async move {
                    deliver_task_notification(&message);
                })
                .await;
        })
        .detach();
    }

    fn record_task(&mut self, t: &TaskInfo, detail: &str, cx: &mut Context<Self>) {
        let record = self.history_record(t, detail);
        self.history.push(record);
        self.save_history(cx);
    }

    /// Checks the queue ten times a second while anything is still running.
    pub(crate) fn ensure_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        this.refresh_tasks(cx);
                        let busy = this.task_view.iter().any(|t| !t.state.is_finished());
                        if !busy {
                            this.polling = false;
                        }
                        busy
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn cancel_task(&mut self, id: TaskId, cx: &mut Context<Self>) {
        self.tasks.cancel(id);
        self.refresh_tasks(cx);
    }

    pub fn toggle_pause(&mut self, id: TaskId, cx: &mut Context<Self>) {
        let paused = self
            .tasks
            .get(id)
            .is_some_and(|t| t.state == TaskState::Paused);
        if paused {
            self.tasks.resume(id);
        } else {
            self.tasks.pause(id);
        }
        self.refresh_tasks(cx);
    }

    pub fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.tasks.clear_finished();
        self.refresh_tasks(cx);
    }

    pub fn hash_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.submit(
            JobSpec::Hash {
                file: path,
                algorithms: vec![
                    rz_app::hash::HashAlgorithm::Sha256,
                    rz_app::hash::HashAlgorithm::Blake3,
                ],
            },
            cx,
        );
    }

    // ----- password prompt ---------------------------------------------------------------

    pub fn ask_password(
        &mut self,
        target: PasswordTarget,
        wrong: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(self.tool_text("password_placeholder"))
                .masked(true)
        });
        self.overlay = Some(Overlay::Password(PasswordPrompt {
            target,
            input,
            wrong,
        }));
        cx.notify();
    }

    pub fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Password(p)) = self.overlay.take() else {
            return;
        };
        let pw = p.input.read(cx).value().to_string();
        if pw.is_empty() {
            self.overlay = Some(Overlay::Password(p));
            return;
        }
        match p.target {
            PasswordTarget::Open(path) => self.open_path(path, Some(pw), window, cx),
            PasswordTarget::Task(id) => {
                if let Some(new) = self.tasks.retry_with_password(id, &pw) {
                    // The retry is a new task; it inherits what the failed one was for.
                    if let Some(path) = self.edits.remove(&id) {
                        self.edits.insert(new, path);
                    }
                    if let Some(path) = self.tests.remove(&id) {
                        self.tests.insert(new, path);
                    }
                    self.refresh_tasks(cx);
                    self.ensure_polling(cx);
                }
            }
        }
        cx.notify();
    }

    // ----- compress form -----------------------------------------------------------------

    pub fn open_compress(
        &mut self,
        sources: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if sources.is_empty() {
            return;
        }
        let stem = if sources.len() == 1 {
            sources[0]
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            sources[0]
                .parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let stem = if stem.is_empty() {
            self.tool_text("default_archive_name").to_string()
        } else {
            stem
        };
        let name = cx.new(|cx| InputState::new(window, cx).default_value(stem));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(self.tool_text("password_optional_placeholder"))
                .masked(true)
        });
        let volume_size = cx.new(|cx| InputState::new(window, cx));
        let dictionary_size = cx.new(|cx| InputState::new(window, cx));
        let solid_block_size = cx.new(|cx| InputState::new(window, cx));
        let format = match self.settings.default_format.as_str() {
            "7z" => Format::SevenZ,
            "tar.zst" => Format::TarZst,
            "tar.xz" => Format::TarXz,
            "tar.gz" => Format::TarGz,
            "tar.bz2" => Format::TarBz2,
            "tar.lz4" => Format::TarLz4,
            _ => Format::Zip,
        };
        self.overlay = Some(Overlay::Compress(CompressForm {
            sources,
            name,
            password,
            volume_size,
            dictionary_size,
            solid_block_size,
            format,
            preset: Preset::Balanced,
        }));
        cx.notify();
    }

    pub fn set_format(&mut self, format: Format, cx: &mut Context<Self>) {
        if let Some(Overlay::Compress(f)) = &mut self.overlay {
            f.format = format;
            if f.preset == Preset::EmailShare && !format.supports_volumes() {
                f.preset = Preset::Balanced;
            }
        }
        cx.notify();
    }

    pub fn set_preset(&mut self, preset: Preset, cx: &mut Context<Self>) {
        if let Some(Overlay::Compress(f)) = &mut self.overlay {
            f.preset = preset;
        }
        cx.notify();
    }

    /// Validates the form and queues the job; shows what is wrong otherwise.
    pub fn submit_compress(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Compress(f)) = &self.overlay else {
            return;
        };
        let name = f.name.read(cx).value().trim().to_string();
        let pw = f.password.read(cx).value().to_string();
        let volume_text = f.volume_size.read(cx).value().trim().to_string();
        let dictionary_text = f.dictionary_size.read(cx).value().trim().to_string();
        let solid_text = f.solid_block_size.read(cx).value().trim().to_string();
        if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
            self.say(
                Notice::Error(self.tool_text("compress_name_invalid").into()),
                cx,
            );
            return;
        }
        let volume_text = if volume_text.is_empty()
            && f.preset == Preset::EmailShare
            && f.format.supports_volumes()
        {
            (EMAIL_SHARE_VOLUME_SIZE >> 20).to_string()
        } else {
            volume_text
        };
        let volume_size = if f.format.supports_volumes() && !volume_text.is_empty() {
            let bytes = volume_text
                .parse::<u64>()
                .ok()
                .filter(|mib| *mib > 0)
                .and_then(|mib| mib.checked_mul(1 << 20));
            let Some(bytes) = bytes else {
                self.say(Notice::Error(self.tool_text("volume_invalid").into()), cx);
                return;
            };
            Some(bytes)
        } else {
            None
        };
        let (dictionary_size, solid_block_size) = if f.format == Format::SevenZ {
            let dictionary_size = if dictionary_text.is_empty() {
                None
            } else {
                let bytes = dictionary_text
                    .parse::<u64>()
                    .ok()
                    .filter(|mib| (1..=64).contains(mib))
                    .and_then(|mib| mib.checked_mul(1 << 20))
                    .and_then(|bytes| u32::try_from(bytes).ok());
                let Some(bytes) = bytes else {
                    self.say(Notice::Error(self.tool_text("dict_invalid").into()), cx);
                    return;
                };
                Some(bytes)
            };
            let solid_block_size = if solid_text.is_empty() {
                None
            } else {
                let bytes = solid_text
                    .parse::<u64>()
                    .ok()
                    .and_then(|mib| mib.checked_mul(1 << 20));
                let Some(bytes) = bytes else {
                    self.say(Notice::Error(self.tool_text("solid_invalid").into()), cx);
                    return;
                };
                Some(bytes)
            };
            (dictionary_size, solid_block_size)
        } else {
            (None, None)
        };
        let dir = f.sources[0]
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // A name that is taken gets a number instead of failing.
        let output = rz_engine::unused_path(&dir.join(format!("{name}.{}", f.format.extension())));
        let Some(format) = OutputFormat::from_path(&output) else {
            self.say(
                Notice::Error(self.tool_text("compress_format_unknown").into()),
                cx,
            );
            return;
        };
        let password = (f.format.supports_password() && !pw.is_empty()).then_some(pw);
        let sources = f.sources.clone();
        let spec = JobSpec::Compress {
            sources: sources.clone(),
            output,
            format,
            opts: CreateOptions {
                level: f.preset.level(),
                password,
                threads: self.settings.threads,
                volume_size,
                dictionary_size,
                solid_block_size,
                portable: f.preset == Preset::Portable,
                ..CreateOptions::default()
            },
        };
        self.overlay = None;
        let scan_id = self.tasks.submit(JobSpec::ScanDuplicates { sources });
        self.duplicate_scans.insert(scan_id, spec);
        self.notice = None;
        self.refresh_tasks(cx);
        self.ensure_polling(cx);
        cx.notify();
    }

    pub fn continue_duplicate_compression(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::DuplicatePrompt { job, .. }) = self.overlay.take() else {
            return;
        };
        self.submit(job, cx);
    }

    pub fn cancel_duplicate_compression(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Some(Overlay::DuplicatePrompt { .. })) {
            return;
        }
        self.overlay = None;
        self.say(
            Notice::Info(self.tool_text("duplicate_cancelled").into()),
            cx,
        );
    }

    pub fn close_overlay(&mut self, cx: &mut Context<Self>) {
        if self.tools.busy {
            return;
        }
        self.overlay = None;
        cx.notify();
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Settings);
        cx.notify();
    }

    /// Tells a first-time user about the install statistics, once.
    pub fn show_first_run_notice(&mut self, cx: &mut Context<Self>) {
        if !self.settings.privacy_notice_seen {
            self.overlay = Some(Overlay::Privacy);
            cx.notify();
        }
    }

    /// Changes one setting, stores it, and redraws.
    pub fn change_settings(&mut self, f: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        let old_language = self.settings.language.clone();
        f(&mut self.settings);
        let language = self.settings.language.clone();
        self.save_settings(cx);
        if language != old_language {
            self.table.update(cx, |table, cx| {
                table.delegate_mut().set_language(&language);
                table.refresh(cx);
            });
            cx.spawn(async move |_, cx| {
                cx.update(|app| crate::set_app_language(&language, app));
            })
            .detach();
        }
        cx.notify();
    }

    pub fn set_theme(&mut self, pick: Theme, window: &mut Window, cx: &mut Context<Self>) {
        self.change_settings(|s| s.theme = pick, cx);
        brand::choose(pick, window, cx);
    }

    /// Switches the accent colour everywhere at once and remembers it.
    pub fn set_accent(&mut self, spec: &str, cx: &mut Context<Self>) {
        let spec = spec.to_string();
        self.change_settings(|s| s.accent = spec.clone(), cx);
        brand::set_accent(&spec, cx);
        brand::apply(cx);
    }

    /// Shows the statistics notice again, from the settings, where it can be read and the
    /// choice made once more.
    pub fn show_privacy_notice(&mut self, cx: &mut Context<Self>) {
        self.privacy_from_settings = true;
        self.overlay = Some(Overlay::Privacy);
        cx.notify();
    }

    /// Closes the notice; `keep_stats` is the user's answer to it. Opened from the
    /// settings, it returns there.
    pub fn answer_privacy(&mut self, keep_stats: bool, cx: &mut Context<Self>) {
        self.change_settings(
            |s| {
                s.send_install_stats = keep_stats;
                s.privacy_notice_seen = true;
            },
            cx,
        );
        self.leave_privacy_notice(cx);
        // The notice has been seen now, so the first report may go out.
        self.report_install(cx);
    }

    /// Back from the notice without answering: to the settings, or away.
    pub fn leave_privacy_notice(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.privacy_from_settings) {
            self.overlay = Some(Overlay::Settings);
            cx.notify();
        } else {
            self.close_overlay(cx);
        }
    }
}

// ----- drawing ---------------------------------------------------------------------------

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has_archive = self.current().is_some();
        v_flex()
            .id("main")
            .size_full()
            .relative()
            .bg(p.win)
            .text_color(p.fg)
            .text_size(px(13.))
            .key_context("RustZip")
            .on_action(cx.listener(|this, _: &OpenArchiveDialog, window, cx| {
                this.pick_archive(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewArchiveDialog, window, cx| {
                this.pick_sources(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.update(cx, |s, cx| s.focus(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseTab, _, cx| {
                this.close_tab(this.active, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
            .on_action(cx.listener(|this, _: &crate::OpenFavorites, _, cx| {
                this.show_page(Page::Favorites, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::OpenPasswordBook, _, cx| {
                this.show_page(Page::Passwords, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::ConvertArchive, window, cx| {
                this.open_transform(false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &crate::RepairArchive, window, cx| {
                this.open_transform(true, window, cx)
            }))
            // The right-click menu of the file list.
            .on_action(cx.listener(|this, _: &RowOpen, window, cx| this.open_picked(window, cx)))
            .on_action(cx.listener(|this, _: &RowExtractTo, window, cx| {
                this.extract_to(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RowExtractHere, _, cx| {
                this.extract_picked_here(cx);
            }))
            .on_action(cx.listener(|this, _: &RowExtractNamed, _, cx| {
                this.extract_picked_named(cx);
            }))
            .on_action(cx.listener(|this, _: &RowCopyPath, _, cx| this.copy_picked_paths(cx)))
            .on_action(cx.listener(|this, _: &RowAddHere, window, cx| {
                this.add_into_picked_folder(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RowRename, window, cx| {
                this.ask_rename_entry(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RowDelete, _, cx| this.ask_delete(cx)))
            .on_action(cx.listener(|this, _: &SelectAllRows, _, cx| this.select_all_rows(cx)))
            .on_action(cx.listener(|this, _: &BlankAdd, window, cx| {
                this.pick_files_to_add(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ExtractAll, window, cx| {
                this.extract(false, window, cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_drop(paths.paths().to_vec(), window, cx);
            }))
            .when(has_archive, |d| d.child(self.render_tab_strip(cx)))
            .when(!has_archive && TABS_IN_TITLEBAR, |d| {
                d.child(self.render_empty_titlebar(cx))
            })
            // The native controls occupy the transparent title bar even without tabs.
            .when(has_archive || self.page != Page::Archive, |d| {
                d.child(self.render_toolbar(cx))
            })
            .child(if has_archive || self.page != Page::Archive {
                self.render_browser(window.viewport_size().width, cx)
                    .into_any_element()
            } else {
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .when(window.viewport_size().width >= px(560.), |d| {
                        d.child(self.render_sidebar(cx))
                    })
                    .child(self.render_empty(cx))
                    .into_any_element()
            })
            .when(self.task_view.iter().any(crate::history::is_live), |d| {
                d.child(self.render_tasks(cx))
            })
            .child(self.render_status(cx))
            .when_some(self.render_overlay(window, cx), |d, o| d.child(o))
            .when_some(self.render_toast(cx), |d, t| d.child(t))
    }
}

impl MainView {
    fn render_empty_titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        h_flex()
            .id("empty-titlebar")
            .h(px(TITLEBAR_HEIGHT))
            .flex_none()
            .pl(px(84.))
            .pr_2()
            .bg(p.panel)
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(self.language_picker(cx, "language-picker-titlebar", false))
            .child(self.settings_button(cx))
    }

    /// One tab per open archive, with a "+" to open another.
    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        h_flex()
            .id("tabs")
            .h(px(TITLEBAR_HEIGHT))
            .px_2()
            // Leave room for the traffic lights that sit in the same row.
            .when(TABS_IN_TITLEBAR, |d| d.pl(px(84.)))
            .gap_1()
            .items_end()
            .flex_none()
            .bg(p.panel)
            .border_b_1()
            .border_color(p.line)
            .children(
                (0..self.archives.len())
                    .map(|ix| self.tab(ix, cx))
                    .collect::<Vec<_>>(),
            )
            .child(
                div().h(px(32.)).flex().items_center().child(
                    Button::new("new-tab")
                        .ghost()
                        .icon(IconName::Plus)
                        .tooltip(self.tool_text("open_archive_tooltip"))
                        .accessibility_label(self.tool_text("open_archive_tooltip"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                ),
            )
            .child(div().flex_1())
            .child(self.language_picker(cx, "language-picker-titlebar", false))
            .child(self.settings_button(cx))
    }

    /// A tab for an archive that is open: click to switch, × to close.
    fn tab(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let on = ix == self.active;
        let a = &self.archives[ix];
        let name = a
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .id(("open", ix))
            .gap_1p5()
            .px_3()
            .h(px(32.))
            .max_w(px(240.))
            .rounded_t(px(8.))
            .cursor_pointer()
            .text_color(if on { p.fg } else { p.fg2 })
            .when(on, |d| d.bg(p.win))
            .hover(|d| d.bg(if on { p.win } else { p.hover }))
            .on_click(cx.listener(move |this, _, _, cx| this.activate_tab(ix, cx)))
            .child(Icon::new(IconName::Inbox).size_4().text_color(if on {
                p.accent
            } else {
                p.fg3
            }))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
            .when(a.password.is_some(), |d| {
                d.child(
                    div()
                        .text_size(px(11.))
                        .text_color(p.fg3)
                        .child(self.tool_text("encrypted_badge")),
                )
            })
            .child(
                div()
                    .id(("close-open", ix))
                    .flex_none()
                    .aria_label(self.tool_text("close_tab"))
                    .text_color(p.fg3)
                    .hover(|d| d.text_color(p.fg))
                    .child(Icon::new(IconName::Close).size_3())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_tab(ix, cx);
                    })),
            )
            .into_any_element()
    }

    /// The buttons follow what the user is doing: files of an archive, or the list of
    /// recent archives.
    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.page {
            Page::Favorites => self.render_locations_toolbar(cx),
            Page::Passwords => self.render_tools_toolbar(cx),
            Page::History => self.render_history_toolbar(cx),
            Page::Archive => self.render_archive_toolbar(cx).into_any_element(),
            Page::Recent => self.render_recent_toolbar(cx).into_any_element(),
            Page::Folder => h_flex()
                .h(px(46.))
                .flex_none()
                .border_b_1()
                .border_color(palette(cx).line)
                .child(self.render_folder_toolbar(cx))
                .child(self.render_folder_view_switch(cx))
                .into_any_element(),
        }
    }

    fn render_archive_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has = self.current().is_some();
        let has_sel = self.current().is_some_and(|a| a.selected.is_some())
            || !self.table.read(cx).delegate().marked().is_empty();
        let editable = self.can_edit();
        // Said when hovering a button that is off because of the format.
        let read_only = self.tool_text("archive_read_only").replace(
            "{format}",
            &self
                .current()
                .map(|a| a.format.to_uppercase())
                .unwrap_or_default(),
        );
        let one_picked = self.picked_paths(cx).len() == 1;
        // Previews open one file at a time.
        let can_preview = self.selected_row().is_some_and(|r| r.kind == RowKind::File)
            && self.table.read(cx).delegate().marked().len() <= 1;
        h_flex()
            .id("toolbar")
            .h(px(46.))
            .px_3()
            .gap_1()
            .items_center()
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            // Extracts what is picked, or everything when nothing is.
            .child(
                Button::new("extract-all")
                    .primary()
                    .icon(gpui_kit::assets::IconName::ArrowDownToLine)
                    .label(self.tool_text("toolbar_extract"))
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, window, cx| this.extract_to(window, cx))),
            )
            .child(
                Button::new("add")
                    .ghost()
                    .icon(IconName::Plus)
                    .label(self.tool_text("toolbar_add"))
                    .disabled(!editable)
                    .when(!editable, |b| b.tooltip(read_only.clone()))
                    .on_click(
                        cx.listener(|this, _, window, cx| this.pick_files_to_add(window, cx)),
                    ),
            )
            .child(
                Button::new("test")
                    .ghost()
                    .icon(IconName::CircleCheck)
                    .label(self.tool_text("toolbar_test"))
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, _, cx| this.start_test(cx))),
            )
            .child(
                Button::new("rename")
                    .ghost()
                    .icon(IconName::Replace)
                    .label(self.tool_text("toolbar_rename"))
                    .disabled(!one_picked || !editable)
                    .when(!editable, |b| b.tooltip(read_only.clone()))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_rename_entry(window, cx))),
            )
            .child(
                Button::new("delete")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label(self.tool_text("toolbar_delete"))
                    .text_color(if has_sel && editable { p.danger } else { p.fg3 })
                    .disabled(!has_sel || !editable)
                    .when(!editable, |b| b.tooltip(read_only))
                    .on_click(cx.listener(|this, _, _, cx| this.ask_delete(cx))),
            )
            .child(div().w(px(1.)).h(px(16.)).mx_1p5().bg(p.line))
            .child(
                Button::new("preview")
                    .ghost()
                    .icon(IconName::Eye)
                    .label(self.tool_text("toolbar_preview"))
                    .disabled(!can_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(px(190.))
                    .child(Input::new(&self.search).id("search")),
            )
    }

    fn settings_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("open-settings")
            .ghost()
            .icon(IconName::Settings)
            .tooltip(self.tool_text("settings_tooltip"))
            .accessibility_label(self.tool_text("settings_tooltip"))
            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)))
    }

    fn language_picker(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        show_name: bool,
    ) -> impl IntoElement {
        let configured = self.settings.language.clone();
        let current = crate::text::language_choice_for(&configured);
        let is_auto = configured.eq_ignore_ascii_case("auto");
        let auto_label = self.tool_text("language_auto").to_string();
        let view = cx.entity().downgrade();
        Button::new(id)
            .ghost()
            .small()
            .icon(IconName::Globe)
            .when(show_name, |button| button.label(current.native_name))
            .when(!show_name, |button| button.label(current.short))
            .tooltip(self.tool_text("language"))
            .accessibility_label(self.tool_text("language"))
            .dropdown_menu(move |mut menu, _, _| {
                let auto_view = view.clone();
                menu = menu.item(
                    PopupMenuItem::new(auto_label.clone())
                        .checked(is_auto)
                        .on_click(move |_, _, app| {
                            let _ = auto_view.update(app, |this, cx| {
                                this.change_settings(
                                    |settings| settings.language = "auto".into(),
                                    cx,
                                );
                            });
                        }),
                );
                for choice in crate::text::LANGUAGE_CHOICES {
                    let selected = !is_auto && configured.eq_ignore_ascii_case(choice.tag);
                    let tag = choice.tag.to_string();
                    let item_view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.native_name)
                            .checked(selected)
                            .on_click(move |_, _, app| {
                                let tag = tag.clone();
                                let _ = item_view.update(app, |this, cx| {
                                    this.change_settings(|settings| settings.language = tag, cx);
                                });
                            }),
                    );
                }
                menu
            })
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        v_flex()
            .id("empty")
            .relative()
            .flex_1()
            .when(!TABS_IN_TITLEBAR, |d| {
                d.child(
                    div().absolute().top_2().right_2().child(
                        h_flex()
                            .gap_1()
                            .child(self.language_picker(cx, "language-picker-titlebar", false))
                            .child(self.settings_button(cx)),
                    ),
                )
            })
            .items_center()
            .justify_center()
            .gap_3()
            .child(img(logo()).size(px(96.)))
            .child(div().text_size(px(28.)).child("RustZIP"))
            .child(
                h_flex()
                    .gap_1p5()
                    .text_size(px(14.))
                    .text_color(p.fg2)
                    .child(Icon::new(IconName::ArrowDown).size_4())
                    .child(self.tool_text("welcome_drag")),
            )
            // Each entry point gets its own full-width row; two buttons side by side
            // looked like one cramped control in the middle of a mostly empty window.
            .child(
                v_flex()
                    .mt_3()
                    .gap_2()
                    .w(px(360.))
                    .child(
                        Button::new("empty-open")
                            .primary()
                            .h(px(44.))
                            .w_full()
                            .text_size(px(15.))
                            .icon(IconName::FolderOpen)
                            .label(self.tool_text("welcome_open_archive"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_archive(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("empty-compress")
                            .outline()
                            .h(px(44.))
                            .w_full()
                            .text_size(px(15.))
                            .icon(IconName::Plus)
                            .label(self.tool_text("welcome_new_archive"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.pick_sources(window, cx)),
                            ),
                    ),
            )
            .when(!self.settings.recent.is_empty(), |d| {
                d.child(
                    v_flex()
                        .mt_4()
                        .gap_0p5()
                        .w(px(360.))
                        .child(
                            div()
                                .text_color(p.fg3)
                                .text_size(px(11.))
                                .child(self.tool_text("welcome_recent")),
                        )
                        .children(
                            self.settings
                                .recent
                                .iter()
                                .take(5)
                                .enumerate()
                                .map(|(ix, path)| self.recent_row(ix, path, cx)),
                        ),
                )
            })
    }

    fn recent_row(&self, ix: usize, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let target = path.to_path_buf();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .id(("recent", ix))
            .gap_2()
            .pl_6()
            .pr_2()
            .h(px(28.))
            .text_size(px(12.))
            .rounded(px(8.))
            .cursor_pointer()
            .hover(|d| d.bg(p.hover))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_path(target.clone(), None, window, cx);
            }))
            .child(Icon::new(IconName::Folder).size_4().text_color(p.accent))
            .child(div().overflow_hidden().text_ellipsis().child(name))
            .into_any_element()
    }

    fn render_browser(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        // `h_flex` centres its children vertically; the three columns must fill the height.
        h_flex()
            .id("browser")
            .flex_1()
            .min_h_0()
            .items_stretch()
            // The sidebar needs room; below this width the file list wins.
            .when(width >= px(560.), |d| d.child(self.render_sidebar(cx)))
            .when(self.page == Page::Recent, |d| {
                d.child(self.render_file_list(false, cx))
            })
            .when(matches!(self.page, Page::Folder | Page::Favorites), |d| {
                d.child(self.render_locations_browser(width, cx))
            })
            .when(self.page == Page::Passwords, |d| {
                d.child(self.render_tools_page(cx))
            })
            .when(self.page == Page::History, |d| {
                d.child(self.render_history_page(cx))
            })
            .when(self.page == Page::Archive, |d| {
                d.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(self.render_breadcrumbs(cx))
                        .child(
                            div()
                                .id("files")
                                .aria_label(self.tool_text("archive_file_list"))
                                .flex_1()
                                .min_h_0()
                                // The table only knows right clicks on rows. Before it sees
                                // one, assume the empty part (an index past the last row); a
                                // click that does land on a row replaces that.
                                .capture_any_mouse_down({
                                    let table = self.table.clone();
                                    move |ev, _, cx| {
                                        if ev.button == MouseButton::Right {
                                            let past = table.read(cx).delegate().table_len();
                                            table.update(cx, |t, cx| {
                                                t.set_right_clicked_row(Some(past), cx)
                                            });
                                        }
                                    }
                                })
                                .relative()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                        this.start_marquee(ev.position, cx);
                                    }),
                                )
                                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                                    this.drag_marquee(ev, cx);
                                }))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.end_marquee(cx)),
                                )
                                .on_mouse_up_out(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| this.end_marquee(cx)),
                                )
                                // Remembers where the list is, to turn mouse positions into rows.
                                .child({
                                    let cell = self.files_bounds.clone();
                                    canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                                        .absolute()
                                        .size_full()
                                })
                                .child(
                                    DataTable::new(&self.table)
                                        .with_size(px(28.))
                                        .bordered(false),
                                )
                                .when_some(self.marquee, |d, band| {
                                    let origin = self.files_bounds.get().origin;
                                    let (x0, x1) = (
                                        band.start.x.min(band.current.x),
                                        band.start.x.max(band.current.x),
                                    );
                                    let (y0, y1) = (
                                        band.start.y.min(band.current.y),
                                        band.start.y.max(band.current.y),
                                    );
                                    d.child(
                                        div()
                                            .absolute()
                                            .left(x0 - origin.x)
                                            .top(y0 - origin.y)
                                            .w(x1 - x0)
                                            .h(y1 - y0)
                                            .border_1()
                                            .border_color(p.accent)
                                            .bg(hsla(0., 0., 0.5, 0.12)),
                                    )
                                }),
                        ),
                )
                // Below this width the file list matters more than the details beside it.
                .when(width >= px(760.), |d| {
                    d.child(
                        v_flex()
                            .id("inspector")
                            .w(px(224.))
                            .flex_none()
                            .p_3()
                            .gap_3()
                            .border_l_1()
                            .border_color(p.line)
                            .child(self.render_inspector(cx)),
                    )
                })
            })
    }

    /// The archives opened lately, as a list like the one in a folder: one line each, with
    /// where it is, when it was made and last changed, and click-to-sort column titles.
    ///
    /// The same list serves the folder page (`folder_mode`), where folders are listed too
    /// and a "back" row stays on top.
    fn render_file_list(&self, folder_mode: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let mono = cx.theme().mono_font_family.clone();
        let list = self.listed();
        let time = |t: Option<i64>| t.map_or_else(|| "—".to_string(), crate::local_time);
        // Column widths; name and place share what is left.
        const W_TIME: f32 = 136.;
        const W_SIZE: f32 = 76.;

        let title = |id: &'static str, label: &'static str, key: RecentSort, right: bool| {
            let on = list.sort == key;
            h_flex()
                .id(id)
                .gap_1()
                .h_full()
                .cursor_pointer()
                .when(right, |d| d.justify_end())
                .text_color(if on { p.fg } else { p.fg3 })
                .hover(|d| d.text_color(p.fg))
                .on_click(cx.listener(move |this, _, _, cx| this.sort_recent(key, cx)))
                .child(label)
                .when(on, |d| {
                    d.child(
                        Icon::new(if list.ascending {
                            IconName::ArrowUp
                        } else {
                            IconName::ArrowDown
                        })
                        .size_3(),
                    )
                })
        };
        let header = h_flex()
            .h(px(32.))
            .px_3()
            .gap_3()
            .flex_none()
            .text_size(px(12.))
            .border_b_1()
            .border_color(p.line)
            .child(
                title(
                    "recent-th-name",
                    self.tool_text("recent_column_name"),
                    RecentSort::Name,
                    false,
                )
                .flex_1()
                .min_w_0(),
            )
            // The size is one of the first things to look for, so it sits next to the name.
            .child(
                title(
                    "recent-th-size",
                    self.tool_text("recent_column_size"),
                    RecentSort::Size,
                    true,
                )
                .w(px(W_SIZE)),
            )
            // In a folder listing every item is in the same place.
            .when(!folder_mode, |d| {
                d.child(
                    title(
                        "recent-th-folder",
                        self.tool_text("recent_column_location"),
                        RecentSort::Folder,
                        false,
                    )
                    .flex_1()
                    .min_w_0()
                    .pl_3(),
                )
            })
            .child(
                title(
                    "recent-th-created",
                    self.tool_text("recent_column_created"),
                    RecentSort::Created,
                    false,
                )
                .w(px(W_TIME)),
            )
            .child(
                title(
                    "recent-th-modified",
                    self.tool_text("recent_column_modified"),
                    RecentSort::Modified,
                    false,
                )
                .w(px(W_TIME)),
            );

        let rows = list
            .items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let on = list.selected.as_ref() == Some(&item.path);
                let open = self.archives.iter().any(|a| a.path == item.path);
                let (single, double) = (item.path.clone(), item.path.clone());
                let is_dir = item.is_dir;
                let dim = if item.exists { p.fg } else { p.fg3 };
                h_flex()
                    .id(("recent", ix))
                    .h(px(36.))
                    .px_3()
                    .gap_3()
                    .flex_none()
                    .cursor_pointer()
                    .text_color(dim)
                    .when(on, |d| d.bg(p.accent_soft))
                    .when(!on, |d| d.hover(|d| d.bg(p.hover)))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        this.select_recent(single.clone(), cx);
                        if ev.click_count() >= 2 {
                            this.open_listed(double.clone(), is_dir, window, cx);
                        }
                    }))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                if item.is_dir {
                                    Icon::new(IconName::Folder)
                                } else {
                                    Icon::new(gpui_kit::assets::IconName::FileArchive)
                                }
                                .size_4()
                                .text_color(if on || item.is_dir { p.accent } else { p.fg3 }),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(item.name.clone()),
                            )
                            .when(open && !item.is_dir, |d| {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(p.fg3)
                                        .child(self.tool_text("recent_item_open")),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w(px(W_SIZE))
                            .text_right()
                            .text_size(px(12.))
                            .font_family(mono.clone())
                            .child(match (item.exists, item.size) {
                                (true, Some(n)) => format_size(n),
                                (true, None) => "—".to_string(),
                                _ => self.tool_text("recent_item_missing").to_string(),
                            }),
                    )
                    .when(!folder_mode, |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pl_3()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(p.fg3)
                                .child(item.folder.clone()),
                        )
                    })
                    .child(
                        div()
                            .w(px(W_TIME))
                            .text_size(px(12.))
                            .font_family(mono.clone())
                            .child(time(item.created)),
                    )
                    .child(
                        div()
                            .w(px(W_TIME))
                            .text_size(px(12.))
                            .font_family(mono.clone())
                            .child(time(item.modified)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        v_flex()
            .id("recent-page")
            .flex_1()
            .min_w_0()
            .child(header)
            // Always on top, whatever the order: one folder up, like `../`.
            .when(folder_mode, |d| {
                let has_parent = self.folder.as_deref().and_then(Path::parent).is_some();
                d.child(
                    h_flex()
                        .id("folder-up")
                        .h(px(36.))
                        .px_3()
                        .gap_2()
                        .flex_none()
                        .items_center()
                        .text_color(p.fg)
                        .when(has_parent, |d| {
                            d.cursor_pointer()
                                .hover(|d| d.bg(p.hover))
                                .on_click(cx.listener(|this, ev: &ClickEvent, _, cx| {
                                    if ev.click_count() >= 2 {
                                        this.folder_up(cx);
                                    }
                                }))
                        })
                        // The same icon and look as the other folders below it.
                        .child(Icon::new(IconName::Folder).size_4().text_color(p.accent))
                        .child("../"),
                )
            })
            .when(rows.is_empty(), |d| {
                d.child(div().p_6().text_color(p.fg3).child(if folder_mode {
                    self.tool_text("recent_empty_folder")
                } else {
                    self.tool_text("recent_empty_list")
                }))
            })
            .child(
                v_flex()
                    .id("recent-rows")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
    }

    /// Toolbar of the recent page: what can be done with the archive picked in the list,
    /// which depends on the archive.
    fn render_recent_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let bar = h_flex()
            .id("toolbar")
            .h(px(46.))
            .px_3()
            .gap_1()
            .items_center()
            .flex_none()
            .border_b_1()
            .border_color(p.line);
        let divider = || div().w(px(1.)).h(px(16.)).mx_1p5().bg(p.line);
        let Some(item) = self.recent.selected_item() else {
            // Nothing picked: the list itself is what there is to work with.
            return bar
                .child(
                    Button::new("recent-open-other")
                        .primary()
                        .icon(IconName::FolderOpen)
                        .label(self.tool_text("welcome_open_archive"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                )
                .child(
                    Button::new("recent-new")
                        .ghost()
                        .icon(IconName::Plus)
                        .label(self.tool_text("welcome_new_archive"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_sources(window, cx))),
                )
                .child(divider())
                .child(
                    Button::new("recent-clear")
                        .ghost()
                        .icon(IconName::Delete)
                        .label(self.tool_text("recent_clear"))
                        .disabled(self.settings.recent.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| this.clear_recent(cx))),
                );
        };
        let (path, exists) = (item.path.clone(), item.exists);
        // A file that is gone can only be taken off the list.
        // A single-file format (.gz and the like) has no folder of files to speak of.
        let several = !matches!(
            rz_formats::OutputFormat::from_path(&path),
            Some(rz_formats::OutputFormat::Single(_))
        );
        let forget = path.clone();
        bar.when(exists, |d| {
            d.child(
                Button::new("recent-open")
                    .primary()
                    .icon(IconName::FolderOpen)
                    .label(self.tool_text("open"))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, window, cx| this.open_path(path.clone(), None, window, cx)
                    })),
            )
            .child(
                Button::new("recent-extract-here")
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowDownToLine)
                    .label(self.tool_text("action_extract_here"))
                    .on_click(cx.listener(|this, _, _, cx| this.extract_recent(false, cx))),
            )
            .when(several, |d| {
                d.child(
                    Button::new("recent-extract-named")
                        .ghost()
                        .icon(IconName::Folder)
                        .label(self.tool_text("action_extract_named"))
                        .on_click(cx.listener(|this, _, _, cx| this.extract_recent(true, cx))),
                )
            })
            .child(
                Button::new("recent-hash")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::ShieldCheck))
                    .label(self.tool_text("hash"))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, _, cx| this.hash_file(path.clone(), cx)
                    })),
            )
            .child(divider())
            .child(
                Button::new("recent-reveal")
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .label(self.tool_text("action_reveal"))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |_, _, _, cx| cx.reveal_path(&path)
                    })),
            )
            .child(
                Button::new("recent-rename")
                    .ghost()
                    .icon(IconName::Replace)
                    .label(self.tool_text("action_rename"))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_rename_file(window, cx))),
            )
            .child(
                Button::new("recent-trash")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label(self.tool_text("delete"))
                    .text_color(p.danger)
                    .on_click(cx.listener(|this, _, _, cx| this.ask_trash(cx))),
            )
            .child(divider())
        })
        .child(
            Button::new("recent-forget")
                .ghost()
                .icon(IconName::Close)
                .label(self.tool_text("action_remove_from_list"))
                .on_click(cx.listener(move |this, _, _, cx| this.forget_recent(&forget, cx))),
        )
    }

    /// Toolbar of the folder page: what can be done with the file or folder picked in the
    /// list, which depends on what it is.
    fn render_folder_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let bar = h_flex()
            .id("toolbar")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .h(px(46.))
            .px_3()
            .gap_1()
            .items_center()
            .border_color(p.line);
        let divider = || div().w(px(1.)).h(px(16.)).mx_1p5().bg(p.line);
        let Some(item) = self.files.selected_item() else {
            return bar
                .child(
                    Button::new("folder-open-other")
                        .primary()
                        .icon(IconName::FolderOpen)
                        .label(self.tool_text("welcome_open_archive"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                )
                .child(
                    Button::new("folder-new")
                        .ghost()
                        .icon(IconName::Plus)
                        .label(self.tool_text("welcome_new_archive"))
                        .on_click(cx.listener(|this, _, window, cx| this.pick_sources(window, cx))),
                );
        };
        let (path, is_dir) = (item.path.clone(), item.is_dir);
        let is_archive = !is_dir && recent::looks_like_archive(&item.name);
        let several = !matches!(
            rz_formats::OutputFormat::from_path(&path),
            Some(rz_formats::OutputFormat::Single(_))
        );
        bar.child(
            Button::new("folder-open")
                .primary()
                .icon(IconName::FolderOpen)
                .label(if is_dir {
                    self.tool_text("action_enter")
                } else {
                    self.tool_text("open")
                })
                .on_click(cx.listener({
                    let path = path.clone();
                    move |this, _, window, cx| this.open_listed(path.clone(), is_dir, window, cx)
                })),
        )
        .when(is_archive, |d| {
            d.child(
                Button::new("folder-extract-here")
                    .ghost()
                    .icon(gpui_kit::assets::IconName::ArrowDownToLine)
                    .label(self.tool_text("action_extract_here"))
                    .on_click(cx.listener(|this, _, _, cx| this.extract_recent(false, cx))),
            )
            .when(several, |d| {
                d.child(
                    Button::new("folder-extract-named")
                        .ghost()
                        .icon(IconName::Folder)
                        .label(self.tool_text("action_extract_named"))
                        .on_click(cx.listener(|this, _, _, cx| this.extract_recent(true, cx))),
                )
            })
        })
        .when(!is_dir, |d| {
            d.child(
                Button::new("folder-hash")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::ShieldCheck))
                    .label(self.tool_text("hash"))
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, _, cx| this.hash_file(path.clone(), cx)
                    })),
            )
        })
        .child(
            Button::new("folder-compress")
                .ghost()
                .icon(IconName::Plus)
                .label(self.tool_text("action_compress"))
                .on_click(cx.listener({
                    let path = path.clone();
                    move |this, _, window, cx| this.open_compress(vec![path.clone()], window, cx)
                })),
        )
        .child(divider())
        .child(
            Button::new("folder-reveal")
                .ghost()
                .icon(IconName::ExternalLink)
                .label(self.tool_text("action_reveal"))
                .on_click(cx.listener({
                    let path = path.clone();
                    move |_, _, _, cx| cx.reveal_path(&path)
                })),
        )
        .child(
            Button::new("folder-rename")
                .ghost()
                .icon(IconName::Replace)
                .label(self.tool_text("action_rename"))
                .on_click(cx.listener(|this, _, window, cx| this.ask_rename_file(window, cx))),
        )
        .child(
            Button::new("folder-trash")
                .ghost()
                .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                .label(self.tool_text("delete"))
                .text_color(p.danger)
                .on_click(cx.listener(|this, _, _, cx| this.ask_trash(cx))),
        )
    }

    fn rename_card(&self, prompt: &RenamePrompt, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (title, lead) = match prompt.target {
            RenameTarget::Entry(_) => (
                self.tool_text("rename_entry_title"),
                self.tool_text("rename_entry_hint"),
            ),
            RenameTarget::File(_) => (
                self.tool_text("rename_archive_title"),
                self.tool_text("rename_archive_hint"),
            ),
        };
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(IconName::Replace),
            title: title.into(),
            lead: Some(lead.into()),
            body: Some(
                v_flex()
                    .gap_2()
                    .child(Input::new(&prompt.input))
                    .when_some(prompt.error.clone(), |d, e| {
                        d.child(div().text_color(p.danger).child(e))
                    })
                    .into_any_element(),
            ),
            buttons: vec![
                Button::new("rename-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("rename-ok")
                    .primary()
                    .icon(IconName::Check)
                    .label(self.tool_text("action_rename"))
                    .on_click(cx.listener(|this, _, window, cx| this.submit_rename(window, cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn trash_card(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Dialog {
            tone: Tone::Danger,
            icon: Icon::new(gpui_kit::assets::IconName::Trash),
            title: if path.is_dir() {
                self.tool_text("trash_folder_title").into()
            } else {
                self.tool_text("trash_file_title").into()
            },
            lead: Some(self.tool_text("trash_hint").into()),
            body: Some(
                div()
                    .px_3()
                    .py_2()
                    .rounded(px(8.))
                    .bg(p.panel)
                    .border_1()
                    .border_color(p.line)
                    .text_size(px(13.))
                    .text_color(p.fg)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name)
                    .into_any_element(),
            ),
            buttons: vec![
                Button::new("trash-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("trash-ok")
                    .danger()
                    .bg(p.danger)
                    .text_color(hsla(0., 0., 1., 1.))
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label(self.tool_text("delete"))
                    .on_click(cx.listener(|this, _, _, cx| this.confirm_trash(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has_archive = self.current().is_some();
        let primary_label = self.tool_text(if has_archive {
            "sidebar_current_archive"
        } else {
            "sidebar_home"
        });
        let primary_accessible_label = self.tool_text(if has_archive {
            "current_archive"
        } else {
            "home"
        });
        let heading = |text: &'static str| {
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .px_2()
                .pt_2()
                .pb_1()
                .text_size(px(11.))
                .text_color(p.fg3)
                .child(text)
        };
        // One row: icon, label and, on the right, a count or "coming soon".
        let mono = cx.theme().mono_font_family.clone();
        let on_recent = self.page == Page::Recent;
        let nav = |icon: Icon,
                   label: &'static str,
                   full_label: &'static str,
                   count: Option<String>,
                   state: Nav| {
            let (fg, icon_fg) = match state {
                Nav::Current => (p.accent, p.accent),
                Nav::Live => (p.fg, p.fg2),
            };
            h_flex()
                .gap_2p5()
                .px_2()
                .h(px(32.))
                .rounded(px(8.))
                .text_color(fg)
                .when(state == Nav::Current, |d| d.bg(p.accent_soft))
                .child(icon.size_4().text_color(icon_fg))
                .child(
                    div()
                        .id(label)
                        .test_support()
                        .aria_label(full_label)
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label),
                )
                .when_some(count, |d, n| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.fg3)
                            .font_family(mono.clone())
                            .child(n),
                    )
                })
        };
        v_flex()
            .id("sidebar")
            .test_support()
            .w(px(168.))
            .flex_none()
            .p_2()
            .gap_0p5()
            .bg(p.panel)
            .border_r_1()
            .border_color(p.line)
            .child(heading(self.tool_text("sidebar_locations")))
            .child(
                nav(
                    if has_archive {
                        Icon::new(IconName::Inbox)
                    } else {
                        Icon::new(gpui_kit::assets::IconName::House)
                    },
                    primary_label,
                    primary_accessible_label,
                    None,
                    if self.page == Page::Archive {
                        Nav::Current
                    } else {
                        Nav::Live
                    },
                )
                .id("nav-current")
                .aria_label(primary_accessible_label)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Archive, cx)))
                .test_support(),
            )
            // A page of its own on the right, not a list squeezed into the menu.
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::Clock),
                    self.tool_text("sidebar_recent"),
                    self.tool_text("welcome_recent"),
                    Some(self.settings.recent.len().to_string()),
                    if on_recent { Nav::Current } else { Nav::Live },
                )
                .id("nav-recent")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Recent, cx))),
            )
            .child(
                nav(
                    Icon::new(IconName::Star),
                    self.tool_text("sidebar_favorites"),
                    self.tool_text("favorites"),
                    None,
                    if self.page == Page::Favorites {
                        Nav::Current
                    } else {
                        Nav::Live
                    },
                )
                .id("nav-favorites")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Favorites, cx))),
            )
            .child(heading(self.tool_text("sidebar_quick_actions")))
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::ArrowLeftRight),
                    self.tool_text("sidebar_convert"),
                    self.tool_text("convert"),
                    None,
                    Nav::Live,
                )
                .id("nav-convert")
                .cursor_pointer()
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_transform(false, window, cx)),
                ),
            )
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::Wrench),
                    self.tool_text("sidebar_repair"),
                    self.tool_text("repair"),
                    None,
                    Nav::Live,
                )
                .id("nav-repair")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.open_transform(true, window, cx))),
            )
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::KeyRound),
                    self.tool_text("sidebar_passwords"),
                    self.tool_text("passwords"),
                    None,
                    if self.page == Page::Passwords {
                        Nav::Current
                    } else {
                        Nav::Live
                    },
                )
                .id("nav-passwords")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Passwords, cx))),
            )
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::ListChecks),
                    self.tool_text("sidebar_history"),
                    self.tool_text("history"),
                    None,
                    if self.page == Page::History {
                        Nav::Current
                    } else {
                        Nav::Live
                    },
                )
                .child(self.history_badges_view(cx))
                .id("nav-history")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::History, cx))),
            )
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(a) = self.current() else {
            return div().into_any_element();
        };
        let crumbs = a.tree.breadcrumbs(&a.cwd);
        let archive_name = a
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .id("breadcrumbs")
            .h(px(36.))
            .px_3()
            .gap_1()
            .items_center()
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            .text_color(p.fg2)
            .child(
                Button::new("up")
                    .ghost()
                    .small()
                    .icon(IconName::ArrowUp)
                    .tooltip(self.tool_text("go_up"))
                    .accessibility_label(self.tool_text("go_up"))
                    .disabled(a.cwd.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_up_or_out(cx))),
            )
            .children(
                crumbs
                    .into_iter()
                    .enumerate()
                    .flat_map(|(ix, (label, path))| {
                        let last = path == a.cwd;
                        let text = if ix == 0 { archive_name.clone() } else { label };
                        let crumb = div()
                            .id(("crumb", ix))
                            .px_1()
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(|d| d.bg(p.hover))
                            .when(last, |d| d.text_color(p.fg))
                            .child(text)
                            .on_click(cx.listener(move |this, _, _, cx| this.navigate(&path, cx)))
                            .into_any_element();
                        let sep = (ix > 0).then(|| {
                            Icon::new(IconName::ChevronRight)
                                .size_3()
                                .text_color(p.fg3)
                                .into_any_element()
                        });
                        sep.into_iter().chain(Some(crumb))
                    }),
            )
            .into_any_element()
    }

    fn render_inspector(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let Some(a) = self.current() else {
            return div().into_any_element();
        };
        let kv = |label: &str, value: String| {
            h_flex()
                .justify_between()
                .gap_3()
                .child(div().text_color(p.fg3).child(label.to_string()))
                .child(
                    div()
                        .text_right()
                        .text_size(px(12.))
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(value),
                )
        };
        let picked = self.table.read(cx).delegate().marked();
        if picked.len() > 1 {
            return v_flex()
                .gap_1()
                .child(
                    div().text_size(px(14.)).child(
                        self.tool_text("selection_count")
                            .replace("{count}", &picked.len().to_string()),
                    ),
                )
                .child(
                    div()
                        .text_color(p.fg3)
                        .child(self.tool_text("selection_extract_hint")),
                )
                .into_any_element();
        }
        match self.selected_row() {
            Some(row) => {
                let kind = match row.kind {
                    RowKind::Dir => self.tool_text("entry_folder").to_string(),
                    RowKind::File => rz_app::preview::type_name(&row.name),
                    RowKind::Link => self.tool_text("entry_link").to_string(),
                };
                let picture = self
                    .preview
                    .as_ref()
                    .filter(|pv| pv.archive == a.path && pv.path == row.path)
                    .and_then(|pv| pv.image.clone());
                let text_preview = self
                    .preview
                    .as_ref()
                    .filter(|pv| pv.archive == a.path && pv.path == row.path)
                    .and_then(|pv| pv.text.clone());
                let (tile_bg, tile_fg) = crate::rows::badge_colors(&row, cx);
                v_flex()
                    .gap_4()
                    // The picture itself for images, otherwise a tile in the file type's colour.
                    .child(
                        div()
                            .id("preview")
                            .h(px(140.))
                            .w_full()
                            .rounded(px(12.))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(tile_bg)
                            .when_some(picture, |d, image| {
                                d.child(img(image).max_w_full().max_h_full())
                            })
                            .when_some(text_preview, |d, text| {
                                d.child(
                                    div()
                                        .id("preview-text-content")
                                        .w_full()
                                        .h_full()
                                        .overflow_y_scroll()
                                        .p_2()
                                        .text_left()
                                        .text_size(px(11.))
                                        .line_height(px(15.))
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_color(p.fg)
                                        .child(text),
                                )
                            })
                            .when(self.preview_is_missing(&row), |d| {
                                d.child(
                                    div()
                                        .size(px(56.))
                                        .rounded(px(14.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(p.win)
                                        .text_color(tile_fg)
                                        .text_size(px(15.))
                                        .child(rz_app::browse::badge_label(&row.name, row.kind)),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(row.name.clone()),
                            )
                            .child(div().text_color(p.fg3).child(kind)),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(kv(
                                self.tool_text("detail_original_size"),
                                format_size(row.size),
                            ))
                            // Folders have no packed size of their own.
                            .when_some(
                                row.packed.filter(|_| row.kind != RowKind::Dir),
                                |d, packed| {
                                    d.child(kv(
                                        self.tool_text("detail_packed_size"),
                                        format_size(packed),
                                    ))
                                },
                            )
                            .when_some(
                                compression_ratio(row.size, row.packed)
                                    .filter(|_| row.kind != RowKind::Dir),
                                |d, r| d.child(kv(self.tool_text("detail_saved"), r)),
                            )
                            .when_some(row.modified, |d, t| {
                                d.child(kv(self.tool_text("detail_modified"), crate::local_time(t)))
                            })
                            .when(row.kind == RowKind::Dir, |d| {
                                d.child(kv(
                                    self.tool_text("detail_file_count"),
                                    row.items.to_string(),
                                ))
                            })
                            .when(row.encrypted, |d| {
                                d.child(kv(
                                    self.tool_text("detail_encrypted"),
                                    self.tool_text("common_yes").into(),
                                ))
                            }),
                    )
                    .child(
                        Button::new("extract-this")
                            .primary()
                            .w_full()
                            .h(px(36.))
                            .icon(gpui_kit::assets::IconName::ArrowDownToLine)
                            .label(self.tool_text("detail_extract_item"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.extract(true, window, cx)),
                            ),
                    )
                    .into_any_element()
            }
            // Nothing picked: describe the archive itself.
            None => v_flex()
                .gap_4()
                .child(
                    div()
                        .h(px(140.))
                        .w_full()
                        .rounded(px(12.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(p.accent_soft)
                        .text_color(p.accent)
                        .child(Icon::new(gpui_kit::assets::IconName::FileArchive).size(px(56.))),
                )
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(px(15.))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(
                                    a.path
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                ),
                        )
                        .child(
                            div().text_color(p.fg3).child(
                                self.tool_text("detail_archive_type")
                                    .replace("{format}", &a.format.to_uppercase()),
                            ),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(kv(
                            self.tool_text("detail_file_count"),
                            group_digits(a.tree.total_files() as u64),
                        ))
                        .child(kv(
                            self.tool_text("detail_original_size"),
                            format_size(a.tree.total_size()),
                        ))
                        .when_some(a.tree.total_packed(), |d, packed| {
                            d.child(kv(
                                self.tool_text("detail_packed_size"),
                                format_size(packed),
                            ))
                        })
                        .when_some(
                            compression_ratio(a.tree.total_size(), a.tree.total_packed()),
                            |d, r| d.child(kv(self.tool_text("detail_saved"), r)),
                        ),
                )
                .child(
                    Button::new("extract-all-side")
                        .primary()
                        .w_full()
                        .h(px(36.))
                        .icon(gpui_kit::assets::IconName::ArrowDownToLine)
                        .label(self.tool_text("detail_extract_all"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.extract(false, window, cx)),
                        ),
                )
                .into_any_element(),
        }
    }

    /// Whether the selected file still needs its type tile, i.e. no picture is ready for it.
    fn preview_is_missing(&self, row: &Row) -> bool {
        !self.preview.as_ref().is_some_and(|pv| {
            pv.path == row.path && self.current().is_some_and(|a| a.path == pv.archive)
        })
    }

    fn render_tasks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        v_flex()
            .id("tasks")
            .flex_none()
            .max_h(px(220.))
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(p.line)
            .bg(p.panel)
            .child(
                div()
                    .text_color(p.fg3)
                    .text_size(px(11.))
                    .child(self.tool_text("status_task")),
            )
            .children(
                self.task_view
                    .iter()
                    .rev()
                    .filter(|t| crate::history::is_live(t))
                    .take(4)
                    .map(|t| self.render_task(t, cx)),
            )
    }

    fn render_task(&self, t: &TaskInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let id = t.id;
        let pct = if t.progress.bytes_total > 0 {
            100.0 * t.progress.bytes_done as f32 / t.progress.bytes_total as f32
        } else if t.state == TaskState::Done {
            100.0
        } else {
            0.0
        };
        let detail = match t.state {
            TaskState::Queued => self.tool_text("task_queued").to_string(),
            TaskState::Paused => self.tool_text("task_paused").to_string(),
            TaskState::Running => {
                let mut s = if t.progress.bytes_total == 0 {
                    self.tool_text("processed")
                        .replace("{bytes}", &format_size(t.progress.bytes_done))
                } else {
                    format!(
                        "{} / {}",
                        format_size(t.progress.bytes_done),
                        format_size(t.progress.bytes_total)
                    )
                };
                if t.speed_bps > 0 {
                    s.push_str(&format!(" · {}", format_speed(t.speed_bps)));
                }
                if let Some(eta) = t.eta_secs {
                    s.push_str(&format!(
                        " · {}",
                        crate::text::tr(&self.settings.language, "task_remaining")
                            .replace("{time}", &format_eta(eta, &self.settings.language))
                    ));
                }
                s
            }
            TaskState::Done => self.tool_text("task_done").to_string(),
            TaskState::Cancelled => self.tool_text("task_cancelled").to_string(),
            TaskState::Failed => t.error.as_ref().map_or_else(
                || self.tool_text("task_failed").to_string(),
                |e| crate::text::task_error(&self.settings.language, e),
            ),
        };
        let needs_password = t
            .error
            .as_ref()
            .is_some_and(|e| e.code == ErrorCode::Password)
            && t.state == TaskState::Failed;
        v_flex()
            .id(("task", id as usize))
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_color(p.fg2)
                            .child(kind_label(t.kind, &self.settings.language)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(t.title.clone()),
                    )
                    .when(needs_password, |d| {
                        d.child(
                            Button::new(("task-pw", id as usize))
                                .primary()
                                .xsmall()
                                .label(self.tool_text("task_enter_password"))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ask_password(PasswordTarget::Task(id), false, window, cx);
                                })),
                        )
                    })
                    .when(
                        t.summary.as_ref().is_some_and(|s| s.recovery.is_some()),
                        |d| {
                            d.child(
                                Button::new(("recovery-report", id as usize))
                                    .outline()
                                    .xsmall()
                                    .label(self.tool_text("report_button"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.overlay = Some(Overlay::RecoveryReport(id));
                                        cx.notify();
                                    })),
                            )
                        },
                    )
                    .when(!t.state.is_finished(), |d| {
                        d.child(
                            Button::new(("task-pause", id as usize))
                                .ghost()
                                .xsmall()
                                .label(if t.state == TaskState::Paused {
                                    self.tool_text("task_continue")
                                } else {
                                    self.tool_text("task_pause")
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.toggle_pause(id, cx)),
                                ),
                        )
                        .child(
                            Button::new(("task-cancel", id as usize))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip(self.tool_text("task_cancel"))
                                .accessibility_label(self.tool_text("task_cancel"))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.cancel_task(id, cx)),
                                ),
                        )
                    }),
            )
            .when(!t.state.is_finished(), |d| {
                d.child(
                    Progress::new(("progress", id as usize))
                        .value(pct)
                        .accessibility_label(
                            self.tool_text("task_progress").replace("{task}", &t.title),
                        ),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(if t.state == TaskState::Failed {
                        p.danger
                    } else {
                        p.fg3
                    })
                    .child(detail),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let left = match (&self.loading, self.current()) {
            (Some(path), _) => div().text_color(p.fg2).child(
                self.tool_text("status_opening").replace(
                    "{name}",
                    &path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            ),
            // The list of recent archives has its own description, not the open archive's.
            (None, _) if matches!(self.page, Page::Favorites | Page::Passwords) => {
                let n = if self.page == Page::Favorites {
                    self.locations.list.items.len()
                } else {
                    self.tools.labels.len()
                };
                div().text_color(p.fg2).child(format!(
                    "{} · {n}",
                    self.tool_text(if self.page == Page::Favorites {
                        "favorites"
                    } else {
                        "passwords"
                    })
                ))
            }
            (None, _) if self.page == Page::Recent => {
                let total = self.recent.items.len();
                let missing = self.recent.items.iter().filter(|i| !i.exists).count();
                div().text_color(p.fg2).child(if missing > 0 {
                    self.tool_text("status_recent_missing")
                        .replace("{total}", &total.to_string())
                        .replace("{missing}", &missing.to_string())
                } else {
                    self.tool_text("status_recent_count")
                        .replace("{total}", &total.to_string())
                })
            }
            (None, _) if self.page == Page::Folder => {
                let n = self.files.items.len();
                let name = self
                    .folder
                    .as_deref()
                    .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "/".into());
                div().text_color(p.fg2).child(
                    self.tool_text("status_folder_count")
                        .replace("{name}", &name)
                        .replace("{count}", &n.to_string()),
                )
            }
            (None, Some(a)) => {
                let t = &a.tree;
                let mono = cx.theme().mono_font_family.clone();
                // Packed size as a share of the original: 34% means it shrank to a third.
                let share = t
                    .total_packed()
                    .filter(|_| t.total_size() > 0)
                    .map(|packed| packed as f64 * 100. / t.total_size() as f64);
                h_flex()
                    .gap_4()
                    .text_color(p.fg2)
                    .when_some(a.verified, |d, ok| {
                        d.child(
                            h_flex()
                                .gap_1p5()
                                .text_color(if ok { p.accent } else { p.danger })
                                .child(
                                    if ok {
                                        Icon::new(gpui_kit::assets::IconName::ShieldCheck)
                                    } else {
                                        Icon::new(IconName::CircleX)
                                    }
                                    .size_4(),
                                )
                                .child(if ok {
                                    self.tool_text("status_check_passed")
                                } else {
                                    self.tool_text("status_check_failed")
                                }),
                        )
                    })
                    .child(
                        div().font_family(mono.clone()).child(
                            self.tool_text("status_entry_count")
                                .replace("{count}", &group_digits(t.total_files() as u64)),
                        ),
                    )
                    .child(
                        div()
                            .font_family(mono.clone())
                            .child(match t.total_packed() {
                                Some(packed) => format!(
                                    "{} → {}",
                                    format_size(t.total_size()),
                                    format_size(packed)
                                ),
                                None => format_size(t.total_size()),
                            }),
                    )
                    .when_some(share, |d, n| {
                        d.child(
                            div()
                                .px_2()
                                .rounded_full()
                                .bg(p.accent_soft)
                                .text_color(p.accent)
                                .font_family(mono)
                                .child(if n < 10. {
                                    self.tool_text("status_compression_ratio")
                                        .replace("{ratio}", &format!("{n:.1}"))
                                } else {
                                    self.tool_text("status_compression_ratio")
                                        .replace("{ratio}", &format!("{n:.0}"))
                                }),
                        )
                    })
            }
            (None, None) => div()
                .text_color(p.fg3)
                .child(self.tool_text("status_ready")),
        };
        h_flex()
            .id("status")
            .h(px(34.))
            .px_4()
            .gap_4()
            .items_center()
            .flex_none()
            .bg(p.panel)
            .border_t_1()
            .border_color(p.line)
            .text_size(px(12.))
            .child(left.flex_1().overflow_hidden().text_ellipsis())
            .when_some(self.update.clone(), |d, state| match state {
                UpdateState::Available(u) => d.child(
                    Button::new("install-update")
                        .ghost()
                        .small()
                        .icon(IconName::ArrowDown)
                        .text_color(p.accent)
                        .label(
                            self.tool_text("status_new_version")
                                .replace("{version}", &u.version.to_string()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                ),
                UpdateState::Downloading(u) => d.child(
                    div().text_color(p.fg2).child(
                        self.tool_text("status_downloading")
                            .replace("{version}", &u.version.to_string()),
                    ),
                ),
            })
            .when_some(
                self.current().filter(|_| self.page == Page::Archive),
                |d, a| {
                    let encrypted = a.tree.entries().iter().any(|e| e.encrypted);
                    d.child(
                        h_flex()
                            .gap_1p5()
                            .text_color(p.fg3)
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(a.format.to_uppercase())
                            .when(encrypted, |d| {
                                d.child("·")
                                    .child(Icon::new(gpui_kit::assets::IconName::Lock).size_3())
                                    .child(self.tool_text("encrypted_badge"))
                            }),
                    )
                },
            )
    }

    fn render_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        // Every dialog is built with `dialog::Dialog`: heading, content, button strip.
        let card = match self.overlay.as_ref()? {
            Overlay::Transform(f) => self.transform_card(f, cx),
            Overlay::PasswordEdit(f) => self.password_edit_card(f, cx),
            Overlay::PasswordDelete(label) => self.password_delete_card(label, cx),
            Overlay::RecoveryReport(id) => self.recovery_card(*id, cx),
            Overlay::Password(prompt) => self.password_card(prompt, cx),
            Overlay::Compress(form) => self.compress_card(form, cx),
            Overlay::DuplicatePrompt {
                job,
                groups,
                scan_incomplete,
            } => self.duplicate_prompt_card(job, groups, *scan_incomplete, cx),
            Overlay::Settings => self.settings_card(cx),
            Overlay::Privacy => self.privacy_card(cx),
            Overlay::ConfirmDelete(paths) => self.delete_card(paths, cx),
            Overlay::Rename(prompt) => self.rename_card(prompt, cx),
            Overlay::ConfirmTrash(path) => self.trash_card(path, cx),
        };
        // A tall dialog (settings on a small window) scrolls its content, not the frame.
        let max_height = window.viewport_size().height - px(48.);
        Some(
            div()
                .id("overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.35))
                .occlude()
                .child(
                    v_flex()
                        .w(px(440.))
                        .max_w_full()
                        .max_h(max_height)
                        .rounded(px(12.))
                        .overflow_hidden()
                        .bg(p.win)
                        .border_1()
                        .border_color(p.line)
                        .shadow_lg()
                        .child(card),
                )
                .into_any_element(),
        )
    }

    fn password_card(&self, prompt: &PasswordPrompt, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::Lock),
            title: self.tool_text("password_dialog_title").into(),
            lead: Some(self.tool_text("password_dialog_lead").into()),
            body: Some(
                v_flex()
                    .gap_2()
                    .when(prompt.wrong, |d| {
                        d.child(
                            div()
                                .text_color(p.danger)
                                .child(self.tool_text("password_wrong")),
                        )
                    })
                    .child(
                        Input::new(&prompt.input)
                            .mask_toggle()
                            .aria_label(self.tool_text("password_placeholder")),
                    )
                    .into_any_element(),
            ),
            buttons: vec![
                Button::new("pw-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("pw-ok")
                    .primary()
                    .icon(IconName::Check)
                    .label(self.tool_text("button_confirm"))
                    .on_click(cx.listener(|this, _, window, cx| this.submit_password(window, cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn delete_card(&self, paths: &[String], cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let kind_of = |path: &str| {
            self.current()
                .and_then(|a| a.tree.find(path))
                .map(|r| r.kind)
        };
        // What is being deleted, in words, and the names under it.
        let title = match paths {
            [one] if kind_of(one) == Some(RowKind::Dir) => {
                self.tool_text("delete_archive_folder").to_string()
            }
            [_] => self.tool_text("delete_archive_file").to_string(),
            many => self
                .tool_text("delete_archive_many")
                .replace("{count}", &many.len().to_string()),
        };
        let shown = paths.iter().take(3).map(|path| {
            div()
                .overflow_hidden()
                .text_ellipsis()
                .child(path.rsplit('/').next().unwrap_or(path).to_string())
        });
        let more = paths.len().saturating_sub(3);
        let names = v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .bg(p.panel)
            .border_1()
            .border_color(p.line)
            .text_size(px(13.))
            .text_color(p.fg)
            .children(shown)
            .when(more > 0, |d| {
                d.child(
                    div().text_color(p.fg3).child(
                        self.tool_text("delete_archive_more")
                            .replace("{count}", &more.to_string()),
                    ),
                )
            });
        let note = div()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(p.fg3)
            .child(self.tool_text("delete_archive_safe"));
        Dialog {
            tone: Tone::Danger,
            icon: Icon::new(gpui_kit::assets::IconName::Trash),
            title,
            lead: Some(self.tool_text("delete_archive_lead").into()),
            body: Some(v_flex().gap_3().child(names).child(note).into_any_element()),
            buttons: vec![
                Button::new("delete-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("delete-ok")
                    .danger()
                    .bg(p.danger)
                    .text_color(hsla(0., 0., 1., 1.))
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label(self.tool_text("delete"))
                    .on_click(cx.listener(|this, _, _, cx| this.confirm_delete(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn settings_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let s = &self.settings;
        let segment = |id: &'static str, label: &'static str, on: bool| {
            let b = Button::new(id).label(label).small();
            if on { b.primary() } else { b.outline() }
        };
        let group = |title: &'static str, body: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_color(p.fg3).text_size(px(12.)).child(title))
                .child(body)
        };
        // One switch per row, with a line that says what turning it on does.
        let toggle = |id: &'static str,
                      row_id: &'static str,
                      title: &'static str,
                      hint: &'static str,
                      on: bool,
                      set: fn(&mut Settings, bool),
                      cx: &mut Context<Self>| {
            h_flex()
                .id(row_id)
                .test_support()
                .gap_3()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().child(title))
                        .child(div().text_size(px(12.)).text_color(p.fg3).child(hint)),
                )
                .child(
                    Switch::new(id)
                        .checked(on)
                        .accessibility_label(title)
                        .on_change(cx.listener(move |this, value: &bool, _, cx| {
                            this.change_settings(|settings| set(settings, *value), cx);
                            cx.stop_propagation();
                        })),
                )
        };
        let (theme, conflict) = (s.theme, s.on_conflict);
        let body =
            v_flex()
                .gap_4()
                .child(group(
                    self.tool_text("language"),
                    self.language_picker(cx, "language-picker-settings", true)
                        .into_any_element(),
                ))
                .child(group(
                    self.tool_text("settings_appearance"),
                    h_flex()
                        .gap_1()
                        .children(
                            [
                                (
                                    "theme-system",
                                    self.tool_text("theme_system"),
                                    Theme::System,
                                ),
                                ("theme-light", self.tool_text("theme_light"), Theme::Light),
                                ("theme-dark", self.tool_text("theme_dark"), Theme::Dark),
                            ]
                            .map(|(id, label, t)| {
                                segment(id, label, theme == t).on_click(cx.listener(
                                    move |this, _, window, cx| this.set_theme(t, window, cx),
                                ))
                            }),
                        )
                        .into_any_element(),
                ))
                .child(group(
                    self.tool_text("settings_accent"),
                    h_flex()
                        .gap_2()
                        .children(
                            brand::accents(p.win.l < 0.5, &s.language)
                                .into_iter()
                                .enumerate()
                                .map(|(ix, (id, label, colour))| {
                                    let on = s.accent == id;
                                    Button::new(("accent", ix))
                                        .ghost()
                                        .xsmall()
                                        .w(px(28.))
                                        .h(px(28.))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(if on { p.fg } else { p.win })
                                        .bg(colour)
                                        .accessibility_label(label)
                                        .toggled(on)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_accent(id, cx);
                                        }))
                                })
                                .collect::<Vec<_>>(),
                        )
                        .into_any_element(),
                ))
                .child(group(
                    self.tool_text("settings_conflict"),
                    h_flex()
                        .gap_1()
                        .children(
                            [
                                ("conflict-rename", "settings_keep_both", OnConflict::Rename),
                                ("conflict-skip", "settings_skip", OnConflict::Skip),
                                ("conflict-replace", "settings_replace", OnConflict::Replace),
                            ]
                            .map(|(id, key, c)| {
                                segment(id, self.tool_text(key), conflict == c).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.change_settings(|s| s.on_conflict = c, cx);
                                    }),
                                )
                            }),
                        )
                        .into_any_element(),
                ))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            toggle(
                                "set-smart",
                                "row-smart",
                                self.tool_text("settings_smart_extract"),
                                self.tool_text("settings_smart_extract_hint"),
                                s.smart_extract,
                                |settings, value| settings.smart_extract = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(|s| s.smart_extract = !s.smart_extract, cx);
                            })),
                        )
                        .child(
                            toggle(
                                "set-open-after-extract",
                                "row-open-after-extract",
                                self.tool_text("open_after_extract"),
                                self.tool_text("open_after_extract_hint"),
                                s.open_folder_when_done,
                                |settings, value| settings.open_folder_when_done = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(
                                    |s| s.open_folder_when_done = !s.open_folder_when_done,
                                    cx,
                                );
                            })),
                        )
                        .child(
                            toggle(
                                "set-task-notifications",
                                "row-task-notifications",
                                self.tool_text("task_notifications"),
                                self.tool_text("task_notifications_hint"),
                                s.task_notifications,
                                |settings, value| settings.task_notifications = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(
                                    |s| s.task_notifications = !s.task_notifications,
                                    cx,
                                );
                            })),
                        )
                        .child(
                            toggle(
                                "set-updates",
                                "row-updates",
                                self.tool_text("settings_check_updates"),
                                self.tool_text("settings_check_updates_hint"),
                                s.check_updates,
                                |settings, value| settings.check_updates = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(|s| s.check_updates = !s.check_updates, cx);
                            })),
                        )
                        .child(
                            toggle(
                                "set-passwords",
                                "row-passwords",
                                self.tool_text("settings_remember_passwords"),
                                self.tool_text("settings_remember_passwords_hint"),
                                s.remember_passwords,
                                |settings, value| settings.remember_passwords = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(
                                    |s| s.remember_passwords = !s.remember_passwords,
                                    cx,
                                );
                            })),
                        )
                        .when(s.remember_passwords, |d| {
                            d.child(
                                h_flex().child(
                                    Button::new("forget-passwords")
                                        .outline()
                                        .small()
                                        .label(self.tool_text("settings_forget_passwords"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.forget_all_passwords(cx);
                                        })),
                                ),
                            )
                        })
                        .child(
                            toggle(
                                "set-stats",
                                "row-stats",
                                self.tool_text("settings_install_stats"),
                                self.tool_text("settings_install_stats_hint"),
                                s.send_install_stats,
                                |settings, value| settings.send_install_stats = value,
                                cx,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(
                                    |s| s.send_install_stats = !s.send_install_stats,
                                    cx,
                                );
                            })),
                        )
                        // To read what is sent, and to answer the question once more.
                        .child(
                            Button::new("show-privacy")
                                .ghost()
                                .small()
                                .icon(Icon::new(gpui_kit::assets::IconName::ShieldCheck))
                                .label(self.tool_text("settings_show_stats_privacy"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_privacy_notice(cx);
                                })),
                        ),
                );
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(IconName::Settings),
            title: self.tool_text("settings_title").into(),
            lead: Some(self.tool_text("settings_hint").into()),
            body: Some(body.into_any_element()),
            buttons: vec![
                Button::new("settings-close")
                    .primary()
                    .icon(IconName::Check)
                    .label(self.tool_text("settings_done"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn privacy_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let again = self.privacy_from_settings;
        // One block of the explanation: what it is about, then the facts.
        let block = |title: &'static str, text: &'static str| {
            v_flex()
                .gap_1()
                .px_3()
                .py_2()
                .rounded(px(8.))
                .bg(p.panel)
                .border_1()
                .border_color(p.line)
                .child(div().text_size(px(12.)).text_color(p.fg3).child(title))
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(20.))
                        .text_color(p.fg)
                        .child(text),
                )
        };
        let body = v_flex()
            .gap_3()
            .child(block(
                self.tool_text("privacy_sent_title"),
                self.tool_text("privacy_sent_fields"),
            ))
            .child(block(
                self.tool_text("privacy_never_title"),
                self.tool_text("privacy_never_fields"),
            ))
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.fg3)
                    .child(self.tool_text("privacy_timing")),
            )
            // Opened again from the settings: say what is chosen now.
            .when(again, |d| {
                d.child(div().text_size(px(13.)).text_color(p.accent).child(
                    if self.settings.send_install_stats {
                        self.tool_text("privacy_choice_joined")
                    } else {
                        self.tool_text("privacy_choice_left")
                    },
                ))
            });
        let mut buttons = Vec::new();
        if again {
            // Reading it again must not force an answer.
            buttons.push(
                Button::new("privacy-back")
                    .outline()
                    .icon(IconName::ArrowLeft)
                    .label(self.tool_text("privacy_return"))
                    .on_click(cx.listener(|this, _, _, cx| this.leave_privacy_notice(cx)))
                    .into_any_element(),
            );
        }
        buttons.push(
            Button::new("privacy-off")
                .outline()
                .icon(IconName::Close)
                .label(self.tool_text("privacy_decline"))
                .on_click(cx.listener(|this, _, _, cx| this.answer_privacy(false, cx)))
                .into_any_element(),
        );
        buttons.push(
            Button::new("privacy-on")
                .primary()
                .icon(IconName::CircleCheck)
                .label(self.tool_text(if again {
                    "privacy_participate"
                } else {
                    "privacy_ok"
                }))
                .on_click(cx.listener(|this, _, _, cx| this.answer_privacy(true, cx)))
                .into_any_element(),
        );
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::ShieldCheck),
            title: self.tool_text("privacy_title").into(),
            lead: Some(self.tool_text("privacy_intro").into()),
            body: Some(body.into_any_element()),
            buttons,
        }
        .render(&p)
    }

    fn duplicate_prompt_card(
        &self,
        _job: &JobSpec,
        groups: &[rz_app::duplicates::DuplicateGroup],
        scan_incomplete: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let (title, lead) = if scan_incomplete {
            (
                self.tool_text("duplicate_scan_limit_title"),
                self.tool_text("duplicate_scan_limit_hint"),
            )
        } else {
            (
                self.tool_text("duplicate_prompt_title"),
                self.tool_text("duplicate_prompt_lead"),
            )
        };
        let rows = groups.iter().take(8).map(|group| {
            let names = group.paths.iter().take(4).map(|path| {
                div()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(duplicate_short_name(path))
            });
            let body = v_flex()
                .gap_1()
                .children(names)
                .when(group.paths.len() > 4, |d| {
                    d.child(
                        div().text_size(px(11.)).text_color(p.fg3).child(
                            self.tool_text("duplicate_file_more")
                                .replace("{count}", &(group.paths.len() - 4).to_string()),
                        ),
                    )
                });
            v_flex()
                .gap_2()
                .p_2()
                .rounded(px(8.))
                .bg(p.panel)
                .border_1()
                .border_color(p.line)
                .child(
                    h_flex()
                        .justify_between()
                        .text_size(px(12.))
                        .text_color(p.fg2)
                        .child(
                            self.tool_text("duplicate_group_files")
                                .replace("{count}", &group.paths.len().to_string()),
                        )
                        .child(format_size(group.size)),
                )
                .child(body)
        });
        let extra_groups = groups.len().saturating_sub(8);
        let body = if scan_incomplete {
            v_flex().into_any_element()
        } else {
            v_flex()
                .gap_2()
                .children(rows)
                .when(extra_groups > 0, |d| {
                    d.child(
                        div().text_size(px(12.)).text_color(p.fg3).child(
                            self.tool_text("duplicate_more")
                                .replace("{count}", &extra_groups.to_string()),
                        ),
                    )
                })
                .into_any_element()
        };
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(IconName::Search),
            title: title.into(),
            lead: Some(lead.into()),
            body: Some(body),
            buttons: vec![
                Button::new("duplicate-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("duplicate_cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_duplicate_compression(cx)))
                    .into_any_element(),
                Button::new("duplicate-continue")
                    .primary()
                    .icon(IconName::Check)
                    .label(self.tool_text("duplicate_continue"))
                    .on_click(cx.listener(|this, _, _, cx| this.continue_duplicate_compression(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn compress_card(&self, form: &CompressForm, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let names: Vec<String> = form
            .sources
            .iter()
            .map(|s| {
                s.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        let segment = |id: &'static str, label: &'static str, on: bool| {
            let b = Button::new(id).label(label).small();
            if on { b.primary() } else { b.outline() }
        };
        let lead = self
            .tool_text("compress_selection")
            .replace("{count}", &names.len().to_string())
            .replace("{names}", &names.join(self.tool_text("list_separator")));
        let body = v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_color(p.fg3)
                            .text_size(px(12.))
                            .child(self.tool_text("compress_name_label")),
                    )
                    .child(
                        Input::new(&form.name).aria_label(self.tool_text("compress_name_label")),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_color(p.fg3)
                            .text_size(px(12.))
                            .child(self.tool_text("compress_format_label")),
                    )
                    .child(h_flex().gap_1().children(Format::ALL.into_iter().map(|f| {
                        let id: &'static str = match f {
                            Format::Zip => "fmt-zip",
                            Format::SevenZ => "fmt-7z",
                            Format::TarZst => "fmt-tar-zst",
                            Format::TarXz => "fmt-tar-xz",
                            Format::TarGz => "fmt-tar-gz",
                            Format::TarBz2 => "fmt-tar-bz2",
                            Format::TarLz4 => "fmt-tar-lz4",
                        };
                        segment(id, f.extension(), form.format == f)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_format(f, cx)))
                    }))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_color(p.fg3)
                            .text_size(px(12.))
                            .child(self.tool_text("compress_preset_label")),
                    )
                    .child(
                        h_flex().gap_1().children(
                            [
                                ("preset-fast", "preset_fast", Preset::Fast),
                                ("preset-balanced", "preset_balanced", Preset::Balanced),
                                ("preset-smallest", "preset_smallest", Preset::Smallest),
                            ]
                            .into_iter()
                            .map(|(id, key, preset)| {
                                segment(id, self.tool_text(key), form.preset == preset).on_click(
                                    cx.listener(move |this, _, _, cx| this.set_preset(preset, cx)),
                                )
                            }),
                        ),
                    ),
            )
            .when(form.format.supports_volumes(), |d| {
                d.child(
                    segment(
                        "preset-email-share",
                        self.tool_text("preset_email_share"),
                        form.preset == Preset::EmailShare,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_preset(Preset::EmailShare, cx)),
                    ),
                )
            })
            .child(
                segment(
                    "preset-portable",
                    self.tool_text("preset_portable"),
                    form.preset == Preset::Portable,
                )
                .on_click(cx.listener(|this, _, _, cx| this.set_preset(Preset::Portable, cx))),
            )
            .when(form.format.supports_password(), |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_color(p.fg3)
                                .text_size(px(12.))
                                .child(self.tool_text("compress_password_label")),
                        )
                        .child(
                            Input::new(&form.password)
                                .mask_toggle()
                                .aria_label(self.tool_text("compress_password_label")),
                        ),
                )
            })
            .when(form.format.supports_volumes(), |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_color(p.fg3)
                                .text_size(px(12.))
                                .child(self.tool_text("volume_size")),
                        )
                        .child(
                            Input::new(&form.volume_size).aria_label(self.tool_text("volume_size")),
                        ),
                )
            })
            .when(form.format == Format::SevenZ, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .text_color(p.fg3)
                                        .text_size(px(12.))
                                        .child(self.tool_text("dict_size")),
                                )
                                .child(
                                    Input::new(&form.dictionary_size)
                                        .aria_label(self.tool_text("dict_size")),
                                ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(
                                    div()
                                        .text_color(p.fg3)
                                        .text_size(px(12.))
                                        .child(self.tool_text("solid_size")),
                                )
                                .child(
                                    Input::new(&form.solid_block_size)
                                        .aria_label(self.tool_text("solid_size")),
                                ),
                        ),
                )
            });
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::FileArchive),
            title: self.tool_text("compress_title").into(),
            lead: Some(lead),
            body: Some(body.into_any_element()),
            buttons: vec![
                Button::new("compress-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label(self.tool_text("compress_cancel"))
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("compress-go")
                    .primary()
                    .icon(IconName::Check)
                    .label(self.tool_text("compress_start"))
                    .on_click(cx.listener(|this, _, _, cx| this.submit_compress(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }
}

/// Starts following the system theme; call once per window.
/// The app icon, decoded once. It is drawn at 96 px, so the 256 px image stays sharp
/// on high-density screens.
fn logo() -> Arc<Image> {
    static LOGO: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
    LOGO.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../../../packaging/icons/hicolor/256.png").to_vec(),
        ))
    })
    .clone()
}

pub fn init_window(window: &mut Window, cx: &mut App) {
    brand::follow_system(window, cx);
}
