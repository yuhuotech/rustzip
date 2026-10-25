//! The task history page and the badges on its menu row.

use crate::brand::palette;
use crate::view::{MainView, Page};
use gpui_kit::component::button::*;
use gpui_kit::component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::history::{History, Outcome, Record};
use rz_app::tasks::{ErrorCode, JobKind, TaskInfo, TaskState};
use std::path::PathBuf;

/// The history file sits next to the settings.
pub fn history_path() -> Option<PathBuf> {
    rz_app::settings::config_dir().map(|d| d.join("history.json"))
}

pub fn kind_key(kind: JobKind) -> &'static str {
    match kind {
        JobKind::Compress => "compress",
        JobKind::Extract => "extract",
        JobKind::Test => "test",
        JobKind::Edit => "edit",
        JobKind::Convert => "convert",
        JobKind::Repair => "repair",
    }
}

/// Keeps the user's home folder out of what is written to disk.
fn redact_home(text: &str) -> String {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.len() > 1 {
        text.replace(&home, "~")
    } else {
        text.to_string()
    }
}

impl MainView {
    /// Builds the record of a task that just ended; `detail` is what the user is told.
    pub(crate) fn history_record(&self, t: &TaskInfo, detail: &str) -> Record {
        let outcome = match t.state {
            TaskState::Done => Outcome::Success,
            TaskState::Cancelled => Outcome::Cancelled,
            _ => Outcome::Failed,
        };
        Record {
            kind: kind_key(t.kind).into(),
            title: t.title.clone(),
            outcome,
            detail: redact_home(detail),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64),
            // Someone already looking at the page needs no reminder.
            seen: self.page == Page::History,
            task: Some(t.id),
        }
    }

    /// Writes the history on a worker; a failed write only costs the record on disk.
    pub(crate) fn save_history(&self, cx: &mut Context<Self>) {
        let Some(path) = self.history_path.clone() else {
            return;
        };
        let history: History = self.history.clone();
        cx.background_executor()
            .spawn(async move {
                static SAVE: std::sync::Mutex<()> = std::sync::Mutex::new(());
                let _turn = SAVE.lock().unwrap_or_else(|p| p.into_inner());
                let _ = history.save(&path);
            })
            .detach();
    }

    pub(crate) fn clear_history(&mut self, cx: &mut Context<Self>) {
        self.history.clear();
        self.save_history(cx);
        cx.notify();
    }

    /// What the menu row says: how many are running, failed unseen, finished unseen.
    pub fn history_badges(&self) -> (usize, usize, usize) {
        let running = self
            .task_view
            .iter()
            .filter(|t| !t.state.is_finished())
            .count();
        let unseen = self.history.unseen();
        (running, unseen.failed, unseen.succeeded)
    }

    /// Small counters on the menu row, like a download manager's: in progress, failed, finished.
    pub(crate) fn history_badges_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (running, failed, succeeded) = self.history_badges();
        let mono = cx.theme().mono_font_family.clone();
        let pill = |n: usize, fg: Hsla, bg: Hsla, tip: &'static str| {
            div()
                .id(tip)
                .px_1p5()
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_size(px(11.))
                .font_family(mono.clone())
                .child(n.to_string())
        };
        h_flex()
            .gap_1()
            .when(running > 0, |d| {
                d.child(pill(
                    running,
                    p.accent,
                    p.accent_soft,
                    self.tool_text("history_running"),
                ))
            })
            .when(failed > 0, |d| {
                d.child(pill(
                    failed,
                    p.win,
                    p.danger,
                    self.tool_text("history_failed"),
                ))
            })
            .when(succeeded > 0, |d| {
                d.child(pill(
                    succeeded,
                    p.fg2,
                    p.hover,
                    self.tool_text("history_succeeded"),
                ))
            })
    }

    pub(crate) fn render_history_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        h_flex()
            .h(px(46.))
            .px_3()
            .gap_2()
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .flex_1()
                    .text_color(p.fg3)
                    .child(self.tool_text("history_hint")),
            )
            .child(
                Button::new("history-clear")
                    .outline()
                    .label(self.tool_text("history_clear"))
                    .disabled(self.history.records.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.clear_history(cx))),
            )
            .into_any_element()
    }

    pub(crate) fn render_history_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let mono = cx.theme().mono_font_family.clone();
        let running: Vec<AnyElement> = self
            .task_view
            .iter()
            .filter(|t| !t.state.is_finished())
            .rev()
            .map(|t| {
                self.history_row(
                    "◌",
                    p.accent,
                    p.accent_soft,
                    t.kind,
                    &t.title,
                    self.tool_text("history_running"),
                    "",
                    String::new(),
                    false,
                    None,
                    &mono,
                    cx,
                )
            })
            .collect();
        let rows: Vec<AnyElement> = self
            .history
            .records
            .iter()
            .map(|r| {
                let (mark, status_fg, status_bg, label) = match r.outcome {
                    Outcome::Success => (
                        "✓",
                        p.accent,
                        p.accent_soft,
                        self.tool_text("history_succeeded"),
                    ),
                    Outcome::Failed => ("!", p.win, p.danger, self.tool_text("history_failed")),
                    Outcome::Cancelled => {
                        ("—", p.fg2, p.hover, self.tool_text("history_cancelled"))
                    }
                };
                let kind = kind_from_key(&r.kind);
                // A repair report is only available while its task is still in this session.
                let report = r.task.filter(|id| {
                    self.task_view.iter().any(|t| {
                        t.id == *id && t.summary.as_ref().is_some_and(|s| s.recovery.is_some())
                    })
                });
                self.history_row(
                    mark,
                    status_fg,
                    status_bg,
                    kind,
                    &r.title,
                    label,
                    &r.detail,
                    crate::local_time(r.at),
                    !r.seen,
                    report,
                    &mono,
                    cx,
                )
            })
            .collect();
        let empty = running.is_empty() && rows.is_empty();
        v_flex()
            .id("history-page")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_1()
            .when(empty, |d| {
                d.child(
                    div()
                        .text_color(p.fg2)
                        .child(self.tool_text("history_empty")),
                )
            })
            .children(running)
            .children(rows)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn history_row(
        &self,
        mark: &'static str,
        status_fg: Hsla,
        status_bg: Hsla,
        kind: JobKind,
        title: &str,
        status: &'static str,
        detail: &str,
        when: String,
        fresh: bool,
        report: Option<rz_app::tasks::TaskId>,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        h_flex()
            .h(px(38.))
            .gap_2()
            .px_2()
            .rounded(px(8.))
            .when(fresh, |d| d.bg(p.accent_soft))
            .child(
                h_flex()
                    .h(px(22.))
                    .gap_1()
                    .px_2()
                    .flex_none()
                    .rounded(px(6.))
                    .bg(status_bg)
                    .text_color(status_fg)
                    .text_size(px(11.))
                    .child(mark)
                    .child(status),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(p.fg3)
                    .child(crate::view::kind_label(kind)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(p.fg2)
                    .child(title.to_string()),
            )
            .when(!detail.is_empty(), |d| {
                d.child(
                    div()
                        .max_w(px(240.))
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(11.))
                        .text_color(p.fg3)
                        .child(detail.to_string()),
                )
            })
            .when_some(report, |d, id| {
                d.child(
                    Button::new(("history-report", id as usize))
                        .outline()
                        .xsmall()
                        .label(self.tool_text("report_button"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.overlay = Some(crate::view::Overlay::RecoveryReport(id));
                            cx.notify();
                        })),
                )
            })
            .when(!when.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(p.fg3)
                        .font_family(mono.clone())
                        .child(when),
                )
            })
            .into_any_element()
    }
}

fn kind_from_key(key: &str) -> JobKind {
    match key {
        "compress" => JobKind::Compress,
        "test" => JobKind::Test,
        "edit" => JobKind::Edit,
        "convert" => JobKind::Convert,
        "repair" => JobKind::Repair,
        _ => JobKind::Extract,
    }
}

/// A task the bottom panel still has something to say about: running, queued, paused,
/// or stopped for a password it is waiting for.
pub fn is_live(t: &TaskInfo) -> bool {
    !t.state.is_finished()
        || (t.state == TaskState::Failed
            && t.error
                .as_ref()
                .is_some_and(|e| e.code == ErrorCode::Password))
}
