//! The main window: tabs, toolbar, folder list, details, tasks and status.
//!
//! All decisions (what a folder contains, how a task is doing) come from `rz-app`;
//! this file decides only where things go on screen.

use crate::brand::{self, palette};
use crate::dialog::{Dialog, Tone};
use crate::recent::{self, RecentList, RecentSort};
use crate::rows::FileRows;
use crate::services::UpdateState;
use crate::{
    BlankAdd, CloseTab, ExtractAll, FocusSearch, NewArchiveDialog, OpenArchiveDialog, OpenSettings,
    RowAddHere, RowCopyPath, RowDelete, RowExtractHere, RowExtractNamed, RowExtractTo, RowOpen,
    RowRename, SelectAllRows,
};
use gpui_kit::component::FocusableExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
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
}

/// How a row of the left menu behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nav {
    /// The page being shown.
    Current,
    /// Works.
    Live,
    /// Designed but not built yet.
    Soon,
}

/// The picture shown in the details panel for the selected file.
pub struct Preview {
    pub archive: PathBuf,
    /// Path of the file inside the archive.
    pub path: String,
    pub image: Arc<Image>,
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
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Zip,
        Format::SevenZ,
        Format::TarZst,
        Format::TarXz,
        Format::TarGz,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Zip => "zip",
            Format::SevenZ => "7z",
            Format::TarZst => "tar.zst",
            Format::TarXz => "tar.xz",
            Format::TarGz => "tar.gz",
        }
    }

    pub fn supports_password(self) -> bool {
        matches!(self, Format::Zip | Format::SevenZ)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Fast,
    Balanced,
    Smallest,
}

impl Preset {
    pub fn level(self) -> u32 {
        match self {
            Preset::Fast => 1,
            Preset::Balanced => 6,
            Preset::Smallest => 9,
        }
    }
}

pub struct CompressForm {
    pub sources: Vec<PathBuf>,
    pub name: Entity<InputState>,
    pub password: Entity<InputState>,
    pub format: Format,
    pub preset: Preset,
}

pub enum Overlay {
    Password(PasswordPrompt),
    Compress(CompressForm),
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
    pub archives: Vec<OpenArchive>,
    pub active: usize,
    pub table: Entity<TableState<FileRows>>,
    pub search: Entity<InputState>,
    pub tasks: TaskManager,
    pub task_view: Vec<TaskInfo>,
    pub settings: Settings,
    pub(crate) settings_path: Option<PathBuf>,
    /// Where remembered passwords live; the system keychain in the real application.
    pub vault: Arc<dyn SecretStore>,
    pub update: Option<crate::services::UpdateState>,
    pub notice: Option<Notice>,
    notice_gen: u64,
    pub overlay: Option<Overlay>,
    pub loading: Option<PathBuf>,
    pub preview: Option<Preview>,
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

fn format_speed(bps: u64) -> String {
    format!("{}/s", format_size(bps))
}

fn format_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} 秒"),
        60..=3599 => format!("{} 分 {} 秒", secs / 60, secs % 60),
        _ => format!("{} 小时 {} 分", secs / 3600, secs % 3600 / 60),
    }
}

fn kind_label(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Compress => "压缩",
        JobKind::Extract => "解压",
        JobKind::Test => "测试",
        JobKind::Edit => "修改",
    }
}

fn explain(e: &Error) -> String {
    match e {
        Error::Password => "需要密码,或密码不正确".into(),
        Error::UnknownFormat => "无法识别这个文件的格式".into(),
        Error::Unsupported(m) => format!("暂不支持:{m}"),
        Error::Corrupt(m) => format!("文件已损坏:{m}"),
        Error::UnsafePath(p) => format!("压缩包里有不安全的路径,已拒绝:{p}"),
        Error::LimitExceeded(m) => format!("超出安全限制,已停止:{m}"),
        Error::Io(e) => format!("读写出错:{e}"),
        Error::Cancelled => "已取消".into(),
    }
}

impl MainView {
    pub fn new(
        settings: Settings,
        settings_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            // Modified clicks build a multi-selection in `FileRows`, not a new single row.
            TableState::new(FileRows::default(), window, cx).select_on_modifier_click(false)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("在压缩包内搜索"));
        cx.subscribe_in(&table, window, Self::on_table_event)
            .detach();
        cx.subscribe_in(&search, window, |this, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                this.apply_view(cx);
            }
        })
        .detach();
        Self {
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
            loading: None,
            preview: None,
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

    fn save_settings(&self) {
        if let Some(path) = &self.settings_path {
            let _ = self.settings.save(path);
        }
    }

    /// Shows a message in the status bar for a few seconds. Errors stay a little longer.
    pub(crate) fn say(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice_gen += 1;
        let gen_now = self.notice_gen;
        let secs = match notice {
            Notice::Info(_) => 6,
            Notice::Error(_) => 12,
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
            .map(|a| recent::archive_stem(&a.path))
            .unwrap_or_default();
        let format = self.current().map(|a| a.format.clone()).unwrap_or_default();
        self.table.update(cx, |t, cx| {
            t.delegate_mut().set_menu_context(editable, &stem, &format);
            t.delegate_mut().show(tree, &cwd, &filter);
            t.refresh(cx);
        });
        cx.notify();
    }

    // ----- opening ---------------------------------------------------------------------

    /// Opens an archive without blocking the window.
    pub fn open_path(
        &mut self,
        path: PathBuf,
        password: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.archives.iter().position(|a| a.path == path) {
            self.active = ix;
            self.page = Page::Archive;
            self.apply_view(cx);
            return;
        }
        self.loading = Some(path.clone());
        self.notice = None;
        cx.notify();
        let (p, pw) = (path.clone(), password.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { load_archive(&p, pw.as_deref()) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.finish_open(path, password, result, window, cx);
            });
        })
        .detach();
    }

    pub fn finish_open(
        &mut self,
        path: PathBuf,
        password: Option<String>,
        result: Result<(ArchiveTree, String), Error>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.loading = None;
        match result {
            Ok((tree, format)) => {
                self.settings.remember(&path);
                self.save_settings();
                if self.settings.remember_passwords
                    && let Some(pw) = &password
                {
                    self.remember_password(&path, pw, cx);
                }
                self.archives.push(OpenArchive {
                    path,
                    tree: Arc::new(tree),
                    format,
                    password,
                    cwd: String::new(),
                    selected: None,
                    verified: None,
                });
                self.active = self.archives.len() - 1;
                self.page = Page::Archive;
                self.overlay = None;
                self.apply_view(cx);
            }
            Err(Error::Password) => {
                // A password saved earlier gets one try before the user is asked.
                if password.is_none() && self.settings.remember_passwords {
                    self.open_with_saved_password(path, window, cx);
                    return;
                }
                let wrong = password.is_some();
                self.ask_password(PasswordTarget::Open(path), wrong, window, cx);
            }
            Err(e) => self.say(Notice::Error(explain(&e)), cx),
        }
    }

    pub fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.archives.len() {
            self.archives.remove(ix);
            self.active = self.active.min(self.archives.len().saturating_sub(1));
            if self.archives.is_empty() {
                // The welcome page has no pages to switch between.
                self.page = Page::Archive;
            }
            self.leave_notice();
            self.apply_view(cx);
        }
    }

    pub fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.archives.len() {
            self.active = ix;
            self.page = Page::Archive;
            self.leave_notice();
            self.apply_view(cx);
        }
    }

    pub fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.leave_notice();
        }
        self.page = page;
        if page == Page::Recent {
            self.refresh_recent(cx);
        }
        cx.notify();
    }

    pub fn pick_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("打开压缩包".into()),
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
            prompt: Some("选择要压缩的文件或文件夹".into()),
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
        self.leave_notice();
        self.page = Page::Folder;
        self.folder = Some(dir.clone());
        self.files.selected = None;
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
                        this.say(Notice::Error(format!("打不开这个文件夹:{e}")), cx);
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
            Page::Folder => &self.files,
            _ => &self.recent,
        }
    }

    fn listed_mut(&mut self) -> &mut RecentList {
        match self.page {
            Page::Folder => &mut self.files,
            _ => &mut self.recent,
        }
    }

    /// Reads the shown list again after something on disk changed.
    fn refresh_listed(&mut self, cx: &mut Context<Self>) {
        match self.page {
            Page::Folder => self.reload_folder(cx),
            Page::Recent => self.refresh_recent(cx),
            Page::Archive => {}
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
            self.say(
                Notice::Info("为了安全,不直接打开程序和脚本;请在文件管理器里确认来源再运行".into()),
                cx,
            );
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
                    self.say(Notice::Info("先在列表里选一个文件或文件夹".into()), cx);
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
            prompt: Some("解压到这里".into()),
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
            self.say(Notice::Info("先在列表里选一个文件或文件夹".into()), cx);
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
        let dir = parent.join(recent::archive_stem(&a.path));
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
        self.say(Notice::Info(format!("已复制 {n} 个路径")), cx);
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
            self.say(Notice::Info("这种格式的压缩包不能修改".into()), cx);
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("添加到压缩包".into()),
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
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("新名称"));
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
            prompt.error = Some("名称不能为空,也不能含有 / \\ : * ? \" < > |".into());
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
                    this.save_settings();
                    this.listed_mut().selected = Some(to);
                    this.refresh_listed(cx);
                    this.say(Notice::Info("已重命名".into()), cx);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => this.say(
                    Notice::Error("这个文件夹里已经有同名文件,换一个名字".into()),
                    cx,
                ),
                Err(e) => this.say(Notice::Error(format!("重命名失败:{e}")), cx),
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
        self.save_settings();
        self.recent.items.retain(|i| i.path != path);
        self.recent.selected = None;
        cx.notify();
    }

    pub fn clear_recent(&mut self, cx: &mut Context<Self>) {
        self.settings.recent.clear();
        self.save_settings();
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
            parent.join(recent::archive_stem(&archive))
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
                    this.say(Notice::Info("已移到废纸篓 / 回收站".into()), cx);
                }
                Err(e) => this.say(Notice::Error(format!("没能删除:{e}")), cx),
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
                    Err(e) => this.say(Notice::Error(explain(&e)), cx),
                };
            });
        })
        .detach();
    }

    /// Unpacks the picked file to a scratch folder and opens it with the program the
    /// system uses for that kind of file.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            self.say(Notice::Info("先在列表里选一个文件".into()), cx);
            return;
        };
        let Some(a) = self.current() else { return };
        let entry = row.entry.and_then(|ix| a.tree.entries().get(ix).cloned());
        let (RowKind::File, Some(entry)) = (row.kind, entry) else {
            self.say(Notice::Info("预览只能用于文件".into()), cx);
            return;
        };
        // Opening a program or script from an archive would run it, which is not a preview.
        if crate::services::is_runnable(&row.name) {
            self.say(
                Notice::Info("为了安全,不直接打开程序和脚本;请解压后确认来源再运行".into()),
                cx,
            );
            return;
        }
        let (archive, password) = (a.path.clone(), a.password.clone());
        let encoding = self.settings.forced_encoding().map(str::to_string);
        let name = crate::services::scratch_name(&row.name);
        // Formats that can be rewritten get their changes saved back; the rest are read-only.
        let writable = rz_app::modify::edit_format(&a.format).is_some();
        let (entry_path, archive_key) = (row.path.clone(), archive.clone());
        self.say(Notice::Info(format!("正在准备 {}…", row.name)), cx);
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
                            Notice::Info("已用系统程序打开;保存后会自动写回压缩包".into()),
                            cx,
                        );
                        this.watch_scratch(path, archive_key, entry_path, cx);
                    } else {
                        this.say(
                            Notice::Info("已用系统程序打开(这种压缩包只读,改动不会写回)".into()),
                            cx,
                        );
                    }
                }
                Err(Error::LimitExceeded(_)) => this.say(
                    Notice::Info("文件太大,不适合预览;请用“解压到…”取出来".into()),
                    cx,
                ),
                Err(e) => this.say(Notice::Error(explain(&e)), cx),
            });
        })
        .detach();
    }

    fn submit(&mut self, spec: JobSpec, cx: &mut Context<Self>) -> TaskId {
        let id = self.tasks.submit(spec);
        self.notice = None;
        self.refresh_tasks(cx);
        self.ensure_polling(cx);
        id
    }

    /// Starts reading the selected picture so the details panel can show it.
    ///
    /// Only pictures of a sensible size are read; everything else keeps the type tile.
    pub fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row() else {
            self.preview = None;
            return;
        };
        let Some(a) = self.current() else { return };
        let format = rz_app::preview::image_format(&row.name);
        let entry = row.entry.and_then(|ix| a.tree.entries().get(ix).cloned());
        let (Some(format), Some(entry), RowKind::File) = (format, entry, row.kind) else {
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
        if row.size > rz_app::preview::MAX_IMAGE_BYTES {
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
                        rz_app::preview::MAX_IMAGE_BYTES,
                    )
                })
                .await;
            // A picture that cannot be read just keeps the type tile.
            let Ok(data) = data else { return };
            let format = match format {
                rz_app::preview::ImageFormat::Png => ImageFormat::Png,
                rz_app::preview::ImageFormat::Jpeg => ImageFormat::Jpeg,
                rz_app::preview::ImageFormat::Gif => ImageFormat::Gif,
                rz_app::preview::ImageFormat::Webp => ImageFormat::Webp,
                rz_app::preview::ImageFormat::Bmp => ImageFormat::Bmp,
            };
            let image = Arc::new(Image::from_bytes(format, data));
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
                    let what = kind_label(t.kind);
                    let msg = match t.kind {
                        JobKind::Test => format!("{what}通过:{} 个条目没有发现问题", s.files),
                        JobKind::Edit => format!("{what}完成:压缩包现在有 {} 项", s.files),
                        _ => format!("{what}完成:{} 个文件,{}", s.files, format_size(s.bytes)),
                    };
                    self.say(Notice::Info(msg), cx);
                    if self.settings.open_folder_when_done && t.kind == JobKind::Extract {
                        cx.reveal_path(&s.output);
                    }
                }
                (TaskState::Failed, Some(err), _) if err.code == ErrorCode::Password => {
                    self.say(Notice::Error(format!("{}需要密码", t.title)), cx);
                }
                (TaskState::Failed, Some(err), _) => {
                    self.say(
                        Notice::Error(format!("{}失败:{}", t.title, err.message)),
                        cx,
                    );
                }
                _ => {}
            }
        }
        cx.notify();
    }

    /// Checks the queue ten times a second while anything is still running.
    fn ensure_polling(&mut self, cx: &mut Context<Self>) {
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
                .placeholder("输入密码")
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
            "归档".to_string()
        } else {
            stem
        };
        let name = cx.new(|cx| InputState::new(window, cx).default_value(stem));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("留空则不加密")
                .masked(true)
        });
        let format = match self.settings.default_format.as_str() {
            "7z" => Format::SevenZ,
            "tar.zst" => Format::TarZst,
            "tar.xz" => Format::TarXz,
            "tar.gz" => Format::TarGz,
            _ => Format::Zip,
        };
        self.overlay = Some(Overlay::Compress(CompressForm {
            sources,
            name,
            password,
            format,
            preset: Preset::Balanced,
        }));
        cx.notify();
    }

    pub fn set_format(&mut self, format: Format, cx: &mut Context<Self>) {
        if let Some(Overlay::Compress(f)) = &mut self.overlay {
            f.format = format;
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
        if name.is_empty() || name.contains(['/', '\\', ':']) || name.starts_with('.') {
            self.say(
                Notice::Error("文件名不能为空,也不能含有 / \\ : 或以点开头".into()),
                cx,
            );
            return;
        }
        let dir = f.sources[0]
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let output = dir.join(format!("{name}.{}", f.format.extension()));
        if output.exists() {
            self.say(
                Notice::Error(format!("已存在同名文件:{}", output.display())),
                cx,
            );
            return;
        }
        let Some(format) = OutputFormat::from_path(&output) else {
            self.say(Notice::Error("无法确定压缩格式".into()), cx);
            return;
        };
        let password = (f.format.supports_password() && !pw.is_empty()).then_some(pw);
        let spec = JobSpec::Compress {
            sources: f.sources.clone(),
            output,
            format,
            opts: CreateOptions {
                level: f.preset.level(),
                password,
                threads: self.settings.threads,
                ..CreateOptions::default()
            },
        };
        self.overlay = None;
        self.submit(spec, cx);
    }

    pub fn close_overlay(&mut self, cx: &mut Context<Self>) {
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
        f(&mut self.settings);
        self.save_settings();
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

    /// Closes the first-run notice; `keep_stats` is the user's answer to it.
    pub fn answer_privacy(&mut self, keep_stats: bool, cx: &mut Context<Self>) {
        self.change_settings(
            |s| {
                s.send_install_stats = keep_stats;
                s.privacy_notice_seen = true;
            },
            cx,
        );
        self.close_overlay(cx);
        // The notice has been seen now, so the first report may go out.
        self.report_install(cx);
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
            // With nothing open the welcome screen has its own buttons; a toolbar of
            // disabled buttons would only be noise.
            .when(has_archive, |d| {
                d.child(self.render_tab_strip(cx))
                    .child(self.render_toolbar(cx))
            })
            .child(if has_archive {
                self.render_browser(window.viewport_size().width, cx)
                    .into_any_element()
            } else {
                self.render_empty(cx).into_any_element()
            })
            .when(!self.task_view.is_empty(), |d| {
                d.child(self.render_tasks(cx))
            })
            .child(self.render_status(cx))
            .when_some(self.render_overlay(window, cx), |d, o| d.child(o))
    }
}

impl MainView {
    /// One tab per open archive, with a "+" to open another.
    fn render_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        h_flex()
            .id("tabs")
            .h(px(38.))
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
                        .tooltip("打开压缩包")
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                ),
            )
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
                d.child(div().text_size(px(11.)).text_color(p.fg3).child("已加密"))
            })
            .child(
                div()
                    .id(("close-open", ix))
                    .flex_none()
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
            Page::Archive => self.render_archive_toolbar(cx).into_any_element(),
            Page::Recent => self.render_recent_toolbar(cx).into_any_element(),
            Page::Folder => self.render_folder_toolbar(cx).into_any_element(),
        }
    }

    fn render_archive_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has = self.current().is_some();
        let has_sel = self.current().is_some_and(|a| a.selected.is_some())
            || !self.table.read(cx).delegate().marked().is_empty();
        let editable = self.can_edit();
        // Said when hovering a button that is off because of the format.
        let read_only = format!(
            "{} 压缩包只读,不能添加、重命名或删除",
            self.current()
                .map(|a| a.format.to_uppercase())
                .unwrap_or_default()
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
                    .icon(IconName::ArrowDown)
                    .label("解压到…")
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, window, cx| this.extract_to(window, cx))),
            )
            .child(
                Button::new("add")
                    .ghost()
                    .icon(IconName::Plus)
                    .label("添加")
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
                    .label("测试")
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, _, cx| this.start_test(cx))),
            )
            .child(
                Button::new("rename")
                    .ghost()
                    .icon(IconName::Replace)
                    .label("重命名")
                    .disabled(!one_picked || !editable)
                    .when(!editable, |b| b.tooltip(read_only.clone()))
                    .on_click(cx.listener(|this, _, window, cx| this.ask_rename_entry(window, cx))),
            )
            .child(
                Button::new("delete")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label("删除")
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
                    .label("预览")
                    .disabled(!can_preview)
                    .on_click(cx.listener(|this, _, _, cx| this.open_selected(cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(px(190.))
                    .child(Input::new(&self.search).id("search")),
            )
            .child(self.settings_button(cx))
    }

    fn settings_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        Button::new("open-settings")
            .ghost()
            .icon(IconName::Settings)
            .tooltip("设置")
            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)))
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        v_flex()
            .id("empty")
            .relative()
            .flex_1()
            .child(
                div()
                    .absolute()
                    .top_2()
                    .right_2()
                    .child(self.settings_button(cx)),
            )
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
                    .child("把压缩包或文件拖到窗口里,或从下面开始"),
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
                            .label("打开压缩包")
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
                            .label("新建压缩")
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
                        .child(div().text_color(p.fg3).text_size(px(11.)).child("最近打开"))
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
            .when(self.page == Page::Folder, |d| {
                d.child(self.render_file_list(true, cx))
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
                                // No focus ring: Ctrl+A would flash one around the whole list.
                                .child(
                                    DataTable::new(&self.table)
                                        .focus_ring(false)
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
                title("recent-th-name", "名称", RecentSort::Name, false)
                    .flex_1()
                    .min_w_0(),
            )
            // The size is one of the first things to look for, so it sits next to the name.
            .child(title("recent-th-size", "大小", RecentSort::Size, true).w(px(W_SIZE)))
            // In a folder listing every item is in the same place.
            .when(!folder_mode, |d| {
                d.child(
                    title("recent-th-folder", "位置", RecentSort::Folder, false)
                        .flex_1()
                        .min_w_0()
                        .pl_3(),
                )
            })
            .child(title("recent-th-created", "创建时间", RecentSort::Created, false).w(px(W_TIME)))
            .child(
                title(
                    "recent-th-modified",
                    "修改时间",
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
                                d.child(div().text_size(px(11.)).text_color(p.fg3).child("已打开"))
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
                                _ => "已移走".to_string(),
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
                    "这个文件夹里没有可显示的内容"
                } else {
                    "还没有打开过压缩包。打开的压缩包会记在这里。"
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
                        .label("打开压缩包…")
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                )
                .child(
                    Button::new("recent-new")
                        .ghost()
                        .icon(IconName::Plus)
                        .label("新建压缩")
                        .on_click(cx.listener(|this, _, window, cx| this.pick_sources(window, cx))),
                )
                .child(divider())
                .child(
                    Button::new("recent-clear")
                        .ghost()
                        .icon(IconName::Delete)
                        .label("清空记录")
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
                    .label("打开")
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |this, _, window, cx| this.open_path(path.clone(), None, window, cx)
                    })),
            )
            .child(
                Button::new("recent-extract-here")
                    .ghost()
                    .icon(IconName::ArrowDown)
                    .label("解压到当前文件夹")
                    .on_click(cx.listener(|this, _, _, cx| this.extract_recent(false, cx))),
            )
            .when(several, |d| {
                d.child(
                    Button::new("recent-extract-named")
                        .ghost()
                        .icon(IconName::Folder)
                        .label("解压到同名文件夹")
                        .on_click(cx.listener(|this, _, _, cx| this.extract_recent(true, cx))),
                )
            })
            .child(divider())
            .child(
                Button::new("recent-reveal")
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .label("在文件夹中显示")
                    .on_click(cx.listener({
                        let path = path.clone();
                        move |_, _, _, cx| cx.reveal_path(&path)
                    })),
            )
            .child(
                Button::new("recent-rename")
                    .ghost()
                    .icon(IconName::Replace)
                    .label("重命名")
                    .on_click(cx.listener(|this, _, window, cx| this.ask_rename_file(window, cx))),
            )
            .child(
                Button::new("recent-trash")
                    .ghost()
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label("删除")
                    .text_color(p.danger)
                    .on_click(cx.listener(|this, _, _, cx| this.ask_trash(cx))),
            )
            .child(divider())
        })
        .child(
            Button::new("recent-forget")
                .ghost()
                .icon(IconName::Close)
                .label("从列表移除")
                .on_click(cx.listener(move |this, _, _, cx| this.forget_recent(&forget, cx))),
        )
    }

    /// Toolbar of the folder page: what can be done with the file or folder picked in the
    /// list, which depends on what it is.
    fn render_folder_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let Some(item) = self.files.selected_item() else {
            return bar
                .child(
                    Button::new("folder-open-other")
                        .primary()
                        .icon(IconName::FolderOpen)
                        .label("打开压缩包…")
                        .on_click(cx.listener(|this, _, window, cx| this.pick_archive(window, cx))),
                )
                .child(
                    Button::new("folder-new")
                        .ghost()
                        .icon(IconName::Plus)
                        .label("新建压缩")
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
                .label(if is_dir { "进入" } else { "打开" })
                .on_click(cx.listener({
                    let path = path.clone();
                    move |this, _, window, cx| this.open_listed(path.clone(), is_dir, window, cx)
                })),
        )
        .when(is_archive, |d| {
            d.child(
                Button::new("folder-extract-here")
                    .ghost()
                    .icon(IconName::ArrowDown)
                    .label("解压到当前文件夹")
                    .on_click(cx.listener(|this, _, _, cx| this.extract_recent(false, cx))),
            )
            .when(several, |d| {
                d.child(
                    Button::new("folder-extract-named")
                        .ghost()
                        .icon(IconName::Folder)
                        .label("解压到同名文件夹")
                        .on_click(cx.listener(|this, _, _, cx| this.extract_recent(true, cx))),
                )
            })
        })
        .child(
            Button::new("folder-compress")
                .ghost()
                .icon(IconName::Plus)
                .label("压缩")
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
                .label("在文件夹中显示")
                .on_click(cx.listener({
                    let path = path.clone();
                    move |_, _, _, cx| cx.reveal_path(&path)
                })),
        )
        .child(
            Button::new("folder-rename")
                .ghost()
                .icon(IconName::Replace)
                .label("重命名")
                .on_click(cx.listener(|this, _, window, cx| this.ask_rename_file(window, cx))),
        )
        .child(
            Button::new("folder-trash")
                .ghost()
                .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                .label("删除")
                .text_color(p.danger)
                .on_click(cx.listener(|this, _, _, cx| this.ask_trash(cx))),
        )
    }

    fn rename_card(&self, prompt: &RenamePrompt, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let (title, lead) = match prompt.target {
            RenameTarget::Entry(_) => ("重命名", "改的是压缩包里的名字,会重新生成压缩包。"),
            RenameTarget::File(_) => ("重命名压缩包", "改的是这个压缩包文件在磁盘上的名字。"),
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
                    .label("取消")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("rename-ok")
                    .primary()
                    .icon(IconName::Check)
                    .label("重命名")
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
                "删除这个文件夹?".into()
            } else {
                "删除这个文件?".into()
            },
            lead: Some("会移到废纸篓(Windows 为回收站),需要时还能恢复。".into()),
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
                    .label("取消")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("trash-ok")
                    .danger()
                    .bg(p.danger)
                    .text_color(hsla(0., 0., 1., 1.))
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label("删除")
                    .on_click(cx.listener(|this, _, _, cx| this.confirm_trash(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let heading = |text: &'static str| {
            div()
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
        let nav = |icon: Icon, label: &'static str, count: Option<String>, state: Nav| {
            let (fg, icon_fg) = match state {
                Nav::Current => (p.accent, p.accent),
                Nav::Live => (p.fg, p.fg2),
                Nav::Soon => (p.fg3, p.fg3),
            };
            h_flex()
                .gap_2p5()
                .px_2()
                .h(px(32.))
                .rounded(px(8.))
                .text_color(fg)
                .when(state == Nav::Current, |d| d.bg(p.accent_soft))
                .child(icon.size_4().text_color(icon_fg))
                .child(div().flex_1().child(label))
                .when_some(count, |d, n| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.fg3)
                            .font_family(mono.clone())
                            .child(n),
                    )
                })
                .when(state == Nav::Soon, |d| {
                    d.child(div().text_size(px(11.)).child("即将推出"))
                })
        };
        v_flex()
            .id("sidebar")
            .w(px(168.))
            .flex_none()
            .p_2()
            .gap_0p5()
            .bg(p.panel)
            .border_r_1()
            .border_color(p.line)
            .child(heading("位置"))
            .child(
                nav(
                    Icon::new(IconName::Inbox),
                    "当前压缩包",
                    None,
                    if self.page == Page::Archive {
                        Nav::Current
                    } else {
                        Nav::Live
                    },
                )
                .id("nav-current")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Archive, cx))),
            )
            // A page of its own on the right, not a list squeezed into the menu.
            .child(
                nav(
                    Icon::new(gpui_kit::assets::IconName::Clock),
                    "最近打开",
                    Some(self.settings.recent.len().to_string()),
                    if on_recent { Nav::Current } else { Nav::Live },
                )
                .id("nav-recent")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.show_page(Page::Recent, cx))),
            )
            .child(nav(Icon::new(IconName::Star), "常用位置", None, Nav::Soon))
            .child(heading("快速操作"))
            // These three are not built yet; they stay in the layout, dimmed and labelled.
            .child(nav(
                Icon::new(gpui_kit::assets::IconName::ArrowLeftRight),
                "转换格式",
                None,
                Nav::Soon,
            ))
            .child(nav(
                Icon::new(gpui_kit::assets::IconName::Wrench),
                "修复压缩包",
                None,
                Nav::Soon,
            ))
            .child(nav(
                Icon::new(gpui_kit::assets::IconName::KeyRound),
                "密码本",
                None,
                Nav::Soon,
            ))
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
                    .tooltip("上一级")
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
                    div()
                        .text_size(px(14.))
                        .child(format!("已选 {} 项", picked.len())),
                )
                .child(div().text_color(p.fg3).child("点“解压所选”只解压这些项目"))
                .into_any_element();
        }
        match self.selected_row() {
            Some(row) => {
                let kind = match row.kind {
                    RowKind::Dir => "文件夹".to_string(),
                    RowKind::File => rz_app::preview::type_name(&row.name),
                    RowKind::Link => "链接".to_string(),
                };
                let picture = self
                    .preview
                    .as_ref()
                    .filter(|pv| pv.archive == a.path && pv.path == row.path)
                    .map(|pv| pv.image.clone());
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
                            .child(kv("原始大小", format_size(row.size)))
                            // Folders have no packed size of their own.
                            .when_some(
                                row.packed.filter(|_| row.kind != RowKind::Dir),
                                |d, packed| d.child(kv("压缩后", format_size(packed))),
                            )
                            .when_some(
                                compression_ratio(row.size, row.packed)
                                    .filter(|_| row.kind != RowKind::Dir),
                                |d, r| d.child(kv("节省", r)),
                            )
                            .when_some(row.modified, |d, t| {
                                d.child(kv("修改时间", crate::local_time(t)))
                            })
                            .when(row.kind == RowKind::Dir, |d| {
                                d.child(kv("文件数", row.items.to_string()))
                            })
                            .when(row.encrypted, |d| d.child(kv("加密", "是".into()))),
                    )
                    .child(
                        Button::new("extract-this")
                            .primary()
                            .w_full()
                            .h(px(36.))
                            .icon(IconName::ArrowDown)
                            .label("解压此项")
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
                            div()
                                .text_color(p.fg3)
                                .child(format!("{} 压缩包", a.format.to_uppercase())),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(kv("文件数", group_digits(a.tree.total_files() as u64)))
                        .child(kv("原始大小", format_size(a.tree.total_size())))
                        .when_some(a.tree.total_packed(), |d, packed| {
                            d.child(kv("压缩后", format_size(packed)))
                        })
                        .when_some(
                            compression_ratio(a.tree.total_size(), a.tree.total_packed()),
                            |d, r| d.child(kv("节省", r)),
                        ),
                )
                .child(
                    Button::new("extract-all-side")
                        .primary()
                        .w_full()
                        .h(px(36.))
                        .icon(IconName::ArrowDown)
                        .label("解压全部")
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
        let any_finished = self.task_view.iter().any(|t| t.state.is_finished());
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
                h_flex()
                    .justify_between()
                    .child(div().text_color(p.fg3).text_size(px(11.)).child("任务"))
                    .when(any_finished, |d| {
                        d.child(
                            Button::new("clear-tasks")
                                .ghost()
                                .xsmall()
                                .label("清除已完成")
                                .on_click(cx.listener(|this, _, _, cx| this.clear_finished(cx))),
                        )
                    }),
            )
            .children(
                self.task_view
                    .iter()
                    .rev()
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
            TaskState::Queued => "等待中".to_string(),
            TaskState::Paused => "已暂停".to_string(),
            TaskState::Running => {
                let mut s = format!(
                    "{} / {}",
                    format_size(t.progress.bytes_done),
                    format_size(t.progress.bytes_total)
                );
                if t.speed_bps > 0 {
                    s.push_str(&format!(" · {}", format_speed(t.speed_bps)));
                }
                if let Some(eta) = t.eta_secs {
                    s.push_str(&format!(" · 剩余 {}", format_eta(eta)));
                }
                s
            }
            TaskState::Done => "完成".to_string(),
            TaskState::Cancelled => "已取消".to_string(),
            TaskState::Failed => t
                .error
                .as_ref()
                .map_or_else(|| "失败".to_string(), |e| e.message.clone()),
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
                    .child(div().text_color(p.fg2).child(kind_label(t.kind)))
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
                                .label("输入密码")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.ask_password(PasswordTarget::Task(id), false, window, cx);
                                })),
                        )
                    })
                    .when(!t.state.is_finished(), |d| {
                        d.child(
                            Button::new(("task-pause", id as usize))
                                .ghost()
                                .xsmall()
                                .label(if t.state == TaskState::Paused {
                                    "继续"
                                } else {
                                    "暂停"
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
                                .tooltip("取消")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.cancel_task(id, cx)),
                                ),
                        )
                    }),
            )
            .when(!t.state.is_finished(), |d| {
                d.child(Progress::new(("progress", id as usize)).value(pct))
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
        let left = match (&self.notice, &self.loading, self.current()) {
            (Some(Notice::Error(m)), _, _) => div().text_color(p.danger).child(m.clone()),
            (Some(Notice::Info(m)), _, _) => div().text_color(p.accent).child(m.clone()),
            (None, Some(path), _) => div().text_color(p.fg2).child(format!(
                "正在打开 {}…",
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            )),
            // The list of recent archives has its own description, not the open archive's.
            (None, None, Some(_)) if self.page == Page::Recent => {
                let total = self.recent.items.len();
                let missing = self.recent.items.iter().filter(|i| !i.exists).count();
                div().text_color(p.fg2).child(if missing > 0 {
                    format!("{total} 个最近的压缩包 · {missing} 个已移走")
                } else {
                    format!("{total} 个最近的压缩包")
                })
            }
            (None, None, Some(_)) if self.page == Page::Folder => {
                let n = self.files.items.len();
                let name = self
                    .folder
                    .as_deref()
                    .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "/".into());
                div().text_color(p.fg2).child(format!("{name} · {n} 项"))
            }
            (None, None, Some(a)) => {
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
                                    "校验通过"
                                } else {
                                    "校验未通过"
                                }),
                        )
                    })
                    .child(
                        div()
                            .font_family(mono.clone())
                            .child(format!("{} 项", group_digits(t.total_files() as u64))),
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
                                    format!("压缩率 {n:.1}%")
                                } else {
                                    format!("压缩率 {n:.0}%")
                                }),
                        )
                    })
            }
            (None, None, None) => div().text_color(p.fg3).child("就绪"),
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
                        .label(format!("新版本 {} · 更新", u.version))
                        .on_click(cx.listener(|this, _, _, cx| this.install_update(cx))),
                ),
                UpdateState::Downloading(u) => d.child(
                    div()
                        .text_color(p.fg2)
                        .child(format!("正在下载 {}…", u.version)),
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
                                    .child("已加密")
                            }),
                    )
                },
            )
    }

    fn render_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        // Every dialog is built with `dialog::Dialog`: heading, content, button strip.
        let card = match self.overlay.as_ref()? {
            Overlay::Password(prompt) => self.password_card(prompt, cx),
            Overlay::Compress(form) => self.compress_card(form, cx),
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
            title: "需要密码".into(),
            lead: Some("这个压缩包已加密,输入密码才能继续。".into()),
            body: Some(
                v_flex()
                    .gap_2()
                    .when(prompt.wrong, |d| {
                        d.child(div().text_color(p.danger).child("密码不正确,请再试一次"))
                    })
                    .child(Input::new(&prompt.input).mask_toggle())
                    .into_any_element(),
            ),
            buttons: vec![
                Button::new("pw-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label("取消")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("pw-ok")
                    .primary()
                    .icon(IconName::Check)
                    .label("确定")
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
            [one] if kind_of(one) == Some(RowKind::Dir) => "删除这个文件夹?".to_string(),
            [_] => "删除这个文件?".to_string(),
            many => format!("删除选中的 {} 项?", many.len()),
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
                    div()
                        .text_color(p.fg3)
                        .child(format!("…以及另外 {more} 项")),
                )
            });
        let note = div()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(p.fg3)
            .child(
                "为了安全,会先在旁边生成一个新的压缩包,检查无误后才替换原文件;中途出错时原文件不受影响。",
            );
        Dialog {
            tone: Tone::Danger,
            icon: Icon::new(gpui_kit::assets::IconName::Trash),
            title,
            lead: Some("会从压缩包里移除,删除后无法撤销。".into()),
            body: Some(v_flex().gap_3().child(names).child(note).into_any_element()),
            buttons: vec![
                Button::new("delete-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label("取消")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("delete-ok")
                    .danger()
                    .bg(p.danger)
                    .text_color(hsla(0., 0., 1., 1.))
                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                    .label("删除")
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
        let toggle = |id: &'static str, title: &'static str, hint: &'static str, on: bool| {
            h_flex()
                .gap_3()
                .items_center()
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().child(title))
                        .child(div().text_size(px(12.)).text_color(p.fg3).child(hint)),
                )
                .child(Switch::new(id).checked(on))
        };
        let (theme, conflict) = (s.theme, s.on_conflict);
        let body =
            v_flex()
                .gap_4()
                .child(group(
                    "外观",
                    h_flex()
                        .gap_1()
                        .children(
                            [
                                ("theme-system", "跟随系统", Theme::System),
                                ("theme-light", "浅色", Theme::Light),
                                ("theme-dark", "深色", Theme::Dark),
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
                    "主色",
                    h_flex()
                        .gap_2()
                        .children(
                            brand::accents(p.win.l < 0.5)
                                .into_iter()
                                .enumerate()
                                .map(|(ix, (id, _, colour))| {
                                    let on = s.accent == id;
                                    div()
                                        .id(("accent", ix))
                                        .size(px(28.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .border_2()
                                        .border_color(if on { p.fg } else { p.win })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_accent(id, cx);
                                        }))
                                        .child(div().size(px(20.)).rounded_full().bg(colour))
                                })
                                .collect::<Vec<_>>(),
                        )
                        .into_any_element(),
                ))
                .child(group(
                    "文件已存在时",
                    h_flex()
                        .gap_1()
                        .children(
                            [
                                ("conflict-ask", "询问", OnConflict::Ask),
                                ("conflict-skip", "跳过", OnConflict::Skip),
                                ("conflict-replace", "覆盖", OnConflict::Replace),
                            ]
                            .map(|(id, label, c)| {
                                segment(id, label, conflict == c).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.change_settings(|s| s.on_conflict = c, cx);
                                    },
                                ))
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
                                "智能解压",
                                "压缩包里有多个散文件时,自动放进以压缩包命名的文件夹",
                                s.smart_extract,
                            )
                            .id("row-smart")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(|s| s.smart_extract = !s.smart_extract, cx);
                            })),
                        )
                        .child(
                            toggle(
                                "set-updates",
                                "自动检查更新",
                                "启动时在后台检查新版本;更新包会验证签名",
                                s.check_updates,
                            )
                            .id("row-updates")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(|s| s.check_updates = !s.check_updates, cx);
                            })),
                        )
                        .child(
                            toggle(
                                "set-passwords",
                                "记住密码",
                                "打开加密压缩包成功后,把密码存进系统钥匙串,下次自动尝试",
                                s.remember_passwords,
                            )
                            .id("row-passwords")
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
                                        .label("清除已保存的密码")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.forget_all_passwords(cx);
                                        })),
                                ),
                            )
                        })
                        .child(
                            toggle(
                                "set-stats",
                                "匿名安装统计",
                                "只上报随机编号、版本、系统和语言,不含文件名或使用行为",
                                s.send_install_stats,
                            )
                            .id("row-stats")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.change_settings(
                                    |s| s.send_install_stats = !s.send_install_stats,
                                    cx,
                                );
                            })),
                        ),
                );
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(IconName::Settings),
            title: "设置".into(),
            lead: Some("改动会立即保存。".into()),
            body: Some(body.into_any_element()),
            buttons: vec![
                Button::new("settings-close")
                    .primary()
                    .icon(IconName::Check)
                    .label("完成")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    fn privacy_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::ShieldCheck),
            title: "关于匿名统计".into(),
            lead: Some(
                "RustZIP 是免费开源软件。为了了解有多少人在用,首次启动时会上报一次匿名信息:\
                 随机编号、软件版本、系统及版本、CPU 架构、界面语言。"
                    .into(),
            ),
            body: Some(
                div()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(p.fg2)
                    .child(
                        "不会收集文件名、路径、文件内容或任何使用行为。发送失败时直接丢弃,\
                         不影响启动。你随时可以在设置里关闭。",
                    )
                    .into_any_element(),
            ),
            buttons: vec![
                Button::new("privacy-off")
                    .outline()
                    .icon(IconName::Close)
                    .label("不参与")
                    .on_click(cx.listener(|this, _, _, cx| this.answer_privacy(false, cx)))
                    .into_any_element(),
                Button::new("privacy-on")
                    .primary()
                    .icon(IconName::CircleCheck)
                    .label("好的")
                    .on_click(cx.listener(|this, _, _, cx| this.answer_privacy(true, cx)))
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
        let lead = format!("已选 {} 项:{}", names.len(), names.join("、"));
        let body = v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_color(p.fg3).text_size(px(12.)).child("文件名"))
                    .child(Input::new(&form.name)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_color(p.fg3).text_size(px(12.)).child("格式"))
                    .child(h_flex().gap_1().children(Format::ALL.into_iter().map(|f| {
                        let id: &'static str = match f {
                            Format::Zip => "fmt-zip",
                            Format::SevenZ => "fmt-7z",
                            Format::TarZst => "fmt-tar-zst",
                            Format::TarXz => "fmt-tar-xz",
                            Format::TarGz => "fmt-tar-gz",
                        };
                        segment(id, f.extension(), form.format == f)
                            .on_click(cx.listener(move |this, _, _, cx| this.set_format(f, cx)))
                    }))),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_color(p.fg3).text_size(px(12.)).child("预设"))
                    .child(
                        h_flex().gap_1().children(
                            [
                                ("preset-fast", "极速", Preset::Fast),
                                ("preset-balanced", "均衡", Preset::Balanced),
                                ("preset-smallest", "最小体积", Preset::Smallest),
                            ]
                            .into_iter()
                            .map(|(id, label, preset)| {
                                segment(id, label, form.preset == preset).on_click(
                                    cx.listener(move |this, _, _, cx| this.set_preset(preset, cx)),
                                )
                            }),
                        ),
                    ),
            )
            .when(form.format.supports_password(), |d| {
                d.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_color(p.fg3).text_size(px(12.)).child("密码"))
                        .child(Input::new(&form.password).mask_toggle()),
                )
            });
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::FileArchive),
            title: "新建压缩".into(),
            lead: Some(lead),
            body: Some(body.into_any_element()),
            buttons: vec![
                Button::new("compress-cancel")
                    .outline()
                    .icon(IconName::Close)
                    .label("取消")
                    .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                    .into_any_element(),
                Button::new("compress-go")
                    .primary()
                    .icon(IconName::Check)
                    .label("开始压缩")
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
