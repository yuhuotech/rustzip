//! The main window: tabs, toolbar, folder list, details, tasks and status.
//!
//! All decisions (what a folder contains, how a task is doing) come from `rz-app`;
//! this file decides only where things go on screen.

use crate::brand::{self, palette};
use crate::rows::{FileRows, badge_chip};
use crate::{CloseTab, ExtractAll, FocusSearch, NewArchiveDialog, OpenArchiveDialog};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::table::{DataTable, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Disableable, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::browse::{ArchiveTree, Row, RowKind, compression_ratio, format_size};
use rz_app::settings::Settings;
use rz_app::tasks::{ErrorCode, JobKind, JobSpec, TaskId, TaskInfo, TaskManager, TaskState};
use rz_core::{CreateOptions, Error};
use rz_formats::OutputFormat;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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
}

pub struct MainView {
    pub archives: Vec<OpenArchive>,
    pub active: usize,
    pub table: Entity<TableState<FileRows>>,
    pub search: Entity<InputState>,
    pub tasks: TaskManager,
    pub task_view: Vec<TaskInfo>,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    pub notice: Option<Notice>,
    pub overlay: Option<Overlay>,
    pub loading: Option<PathBuf>,
    polling: bool,
    /// Tasks already reported, so each result is announced once.
    announced: HashSet<TaskId>,
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
        let table = cx.new(|cx| TableState::new(FileRows::default(), window, cx));
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
            notice: None,
            overlay: None,
            loading: None,
            polling: false,
            announced: HashSet::new(),
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

    fn say(&mut self, notice: Notice, cx: &mut Context<Self>) {
        self.notice = Some(notice);
        cx.notify();
    }

    /// Pushes the active archive, folder and search text into the list.
    pub fn apply_view(&mut self, cx: &mut Context<Self>) {
        let filter = self.search_text(cx);
        let (tree, cwd) = match self.current() {
            Some(a) => (Some(a.tree.clone()), a.cwd.clone()),
            None => (None, String::new()),
        };
        self.table.update(cx, |t, cx| {
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
                self.archives.push(OpenArchive {
                    path,
                    tree: Arc::new(tree),
                    format,
                    password,
                    cwd: String::new(),
                    selected: None,
                });
                self.active = self.archives.len() - 1;
                self.overlay = None;
                self.apply_view(cx);
            }
            Err(Error::Password) => {
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
            self.apply_view(cx);
        }
    }

    pub fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.archives.len() {
            self.active = ix;
            self.apply_view(cx);
        }
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
        self.apply_view(cx);
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(a) = self.current() {
            let parent = rz_app::browse::parent_dir(&a.cwd);
            self.navigate(&parent, cx);
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
            TableEvent::SelectRow(ix) => {
                if let Some(row) = row_at(*ix, cx) {
                    if let Some(a) = self.current_mut() {
                        a.selected = Some(row.path);
                    }
                    cx.notify();
                }
            }
            TableEvent::DoubleClickedRow(ix) => {
                if let Some(row) = row_at(*ix, cx)
                    && row.kind == RowKind::Dir
                {
                    // Leaving a search by opening a folder also clears the search.
                    self.search.update(cx, |s, cx| s.set_value("", window, cx));
                    self.navigate(&row.path, cx);
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
            match &a.selected {
                Some(p) => Some(vec![p.clone()]),
                None => {
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
        self.submit(spec, cx);
    }

    fn submit(&mut self, spec: JobSpec, cx: &mut Context<Self>) {
        self.tasks.submit(spec);
        self.notice = None;
        self.refresh_tasks(cx);
        self.ensure_polling(cx);
    }

    /// Re-reads the task list and announces tasks that just ended.
    pub fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        self.task_view = self.tasks.snapshot();
        for t in self.task_view.clone() {
            if !t.state.is_finished() || !self.announced.insert(t.id) {
                continue;
            }
            match (&t.state, &t.error, &t.summary) {
                (TaskState::Done, _, Some(s)) => {
                    let what = kind_label(t.kind);
                    let msg = match t.kind {
                        JobKind::Test => format!("{what}通过:{} 个条目没有发现问题", s.files),
                        _ => format!("{what}完成:{} 个文件,{}", s.files, format_size(s.bytes)),
                    };
                    self.notice = Some(Notice::Info(msg));
                    if self.settings.open_folder_when_done && t.kind == JobKind::Extract {
                        cx.reveal_path(&s.output);
                    }
                }
                (TaskState::Failed, Some(err), _) if err.code == ErrorCode::Password => {
                    self.notice = Some(Notice::Error(format!("{}需要密码", t.title)));
                }
                (TaskState::Failed, Some(err), _) => {
                    self.notice = Some(Notice::Error(format!("{}失败:{}", t.title, err.message)));
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
                if self.tasks.retry_with_password(id, &pw).is_some() {
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
            .on_action(cx.listener(|this, _: &ExtractAll, window, cx| {
                this.extract(false, window, cx);
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_drop(paths.paths().to_vec(), window, cx);
            }))
            // With nothing open the welcome screen has its own buttons; a toolbar of
            // disabled buttons would only be noise.
            .when(has_archive, |d| d.child(self.render_toolbar(cx)))
            .child(if has_archive {
                self.render_browser(cx).into_any_element()
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
    /// A row in the sidebar for an archive that is open: click to switch, × to close.
    fn open_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let on = ix == self.active;
        let name = self.archives[ix]
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        h_flex()
            .id(("open", ix))
            .gap_1p5()
            .px_2()
            .h(px(26.))
            .rounded(px(6.))
            .cursor_pointer()
            .text_color(if on { p.accent } else { p.fg })
            .when(on, |d| d.bg(p.accent_soft))
            .hover(|d| d.bg(if on { p.accent_soft } else { p.hover }))
            .on_click(cx.listener(move |this, _, _, cx| this.activate_tab(ix, cx)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(name),
            )
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

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let has = self.current().is_some();
        let has_sel = self.current().is_some_and(|a| a.selected.is_some());
        h_flex()
            .id("toolbar")
            .h(px(46.))
            .px_3()
            .gap_1()
            .items_center()
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            .child(
                Button::new("extract-all")
                    .primary()
                    .icon(IconName::ArrowDown)
                    .label("解压全部")
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, window, cx| this.extract(false, window, cx))),
            )
            .child(
                Button::new("extract-selected")
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .label("解压所选")
                    .disabled(!has_sel)
                    .on_click(cx.listener(|this, _, window, cx| this.extract(true, window, cx))),
            )
            .child(
                Button::new("test")
                    .ghost()
                    .icon(IconName::CircleCheck)
                    .label("测试")
                    .disabled(!has)
                    .on_click(cx.listener(|this, _, _, cx| this.start_test(cx))),
            )
            .child(div().w(px(1.)).h(px(16.)).mx_1p5().bg(p.line))
            .child(
                Button::new("compress")
                    .ghost()
                    .icon(IconName::Plus)
                    .label("新建压缩")
                    .on_click(cx.listener(|this, _, window, cx| this.pick_sources(window, cx))),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w(px(190.))
                    .child(Input::new(&self.search).id("search")),
            )
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        v_flex()
            .id("empty")
            .flex_1()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .size(px(64.))
                    .rounded(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.accent_soft)
                    .text_color(p.accent)
                    .child(Icon::new(IconName::Inbox).size_8()),
            )
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
            .px_2()
            .py_1p5()
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

    fn render_browser(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        // `h_flex` centres its children vertically; the three columns must fill the height.
        h_flex()
            .id("browser")
            .flex_1()
            .min_h_0()
            .items_stretch()
            // The sidebar only earns its width when there is something to switch between.
            .when(self.archives.len() > 1, |d| {
                d.child(self.render_sidebar(cx))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(self.render_breadcrumbs(cx))
                    .child(
                        div().id("files").flex_1().min_h_0().child(
                            DataTable::new(&self.table)
                                .with_size(px(28.))
                                .bordered(false),
                        ),
                    ),
            )
            .child(
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
        v_flex()
            .id("sidebar")
            .w(px(168.))
            .flex_none()
            .p_2()
            .gap_0p5()
            .bg(p.panel)
            .border_r_1()
            .border_color(p.line)
            // Several archives can be open at once; they switch here instead of in a tab strip.
            .when(self.archives.len() > 1, |d| {
                d.child(heading("已打开")).children(
                    (0..self.archives.len())
                        .map(|ix| self.open_row(ix, cx))
                        .collect::<Vec<_>>(),
                )
            })
            .child(heading("最近打开"))
            .children(
                self.settings
                    .recent
                    .iter()
                    .take(8)
                    .enumerate()
                    .map(|(ix, path)| self.recent_row(ix, path, cx))
                    .collect::<Vec<_>>(),
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
                    .tooltip("上一级")
                    .disabled(a.cwd.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_up(cx))),
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
        match self.selected_row() {
            Some(row) => {
                let kind = match row.kind {
                    RowKind::Dir => "文件夹",
                    RowKind::File => "文件",
                    RowKind::Link => "链接",
                };
                v_flex()
                    .gap_4()
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .child(badge_chip(&row, cx))
                            .child(div().text_size(px(14.)).child(row.name.clone())),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(kv("类型", kind.to_string()))
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
                            .label("解压此项")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.extract(true, window, cx)),
                            ),
                    )
                    .into_any_element()
            }
            None => v_flex()
                .gap_3()
                .child(
                    div()
                        .text_color(p.fg3)
                        .child("选择一个文件或文件夹查看详情"),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .child(kv("格式", a.format.clone()))
                        .child(kv("文件数", a.tree.total_files().to_string()))
                        .child(kv("原始大小", format_size(a.tree.total_size())))
                        .when_some(a.tree.total_packed(), |d, packed| {
                            d.child(kv("压缩后", format_size(packed)))
                        }),
                )
                .into_any_element(),
        }
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
            (None, None, Some(a)) => {
                let t = &a.tree;
                let mut s = format!(
                    "{} 个文件 · {}",
                    t.total_files(),
                    format_size(t.total_size())
                );
                if let Some(packed) = t.total_packed() {
                    s.push_str(&format!(" → {}", format_size(packed)));
                    if let Some(r) = compression_ratio(t.total_size(), Some(packed)) {
                        s.push_str(&format!(" · 节省 {r}"));
                    }
                }
                div().text_color(p.fg2).child(s)
            }
            (None, None, None) => div().text_color(p.fg3).child("就绪"),
        };
        h_flex()
            .id("status")
            .h(px(28.))
            .px_3()
            .gap_4()
            .items_center()
            .flex_none()
            .bg(p.panel)
            .border_t_1()
            .border_color(p.line)
            .text_size(px(12.))
            .child(left.flex_1().overflow_hidden().text_ellipsis())
            .when_some(self.current(), |d, a| {
                d.child(
                    div()
                        .text_color(p.fg3)
                        .font_family(cx.theme().mono_font_family.clone())
                        .child(a.format.to_uppercase()),
                )
            })
    }

    fn render_overlay(&self, _: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = palette(cx);
        let card = match self.overlay.as_ref()? {
            Overlay::Password(prompt) => self.password_card(prompt, cx).into_any_element(),
            Overlay::Compress(form) => self.compress_card(form, cx).into_any_element(),
        };
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
                        .w(px(420.))
                        .max_w_full()
                        .p_4()
                        .gap_3()
                        .rounded(px(12.))
                        .bg(p.win)
                        .border_1()
                        .border_color(p.line)
                        .shadow_lg()
                        .child(card),
                )
                .into_any_element(),
        )
    }

    fn password_card(&self, prompt: &PasswordPrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        v_flex()
            .gap_3()
            .child(div().text_size(px(15.)).child("需要密码"))
            .when(prompt.wrong, |d| {
                d.child(div().text_color(p.danger).child("密码不正确,请再试一次"))
            })
            .child(Input::new(&prompt.input).mask_toggle())
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("pw-cancel")
                            .outline()
                            .label("取消")
                            .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx))),
                    )
                    .child(Button::new("pw-ok").primary().label("确定").on_click(
                        cx.listener(|this, _, window, cx| this.submit_password(window, cx)),
                    )),
            )
    }

    fn compress_card(&self, form: &CompressForm, cx: &mut Context<Self>) -> impl IntoElement {
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
        v_flex()
            .gap_4()
            .child(div().text_size(px(16.)).child("新建压缩"))
            .child(div().text_color(p.fg3).child(format!(
                "已选 {} 项:{}",
                names.len(),
                names.join("、")
            )))
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
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("compress-cancel")
                            .outline()
                            .label("取消")
                            .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx))),
                    )
                    .child(
                        Button::new("compress-go")
                            .primary()
                            .label("开始压缩")
                            .on_click(cx.listener(|this, _, _, cx| this.submit_compress(cx))),
                    ),
            )
    }
}

/// Starts following the system theme; call once per window.
pub fn init_window(window: &mut Window, cx: &mut App) {
    brand::follow_system(window, cx);
}
