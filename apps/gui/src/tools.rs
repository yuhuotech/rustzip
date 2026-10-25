//! Sidebar tools. Disk and keychain access always run on the background executor.

use crate::brand::palette;
use crate::dialog::{Dialog, Tone};
use crate::view::{MainView, Notice, Overlay};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::{JobSpec, OutputFormat, TaskId, TransformOptions};
use std::path::PathBuf;
use zeroize::Zeroizing;

const FORMATS: [&str; 7] = ["zip", "7z", "tar", "tar.gz", "tar.xz", "tar.zst", "tar.bz2"];

#[derive(Default)]
pub struct ToolState {
    pub labels: Vec<String>,
    pub busy: bool,
    pub loading: bool,
    pub error: Option<String>,
    generation: u64,
    pub(crate) settings_revision: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

pub struct PasswordForm {
    pub original: Option<String>,
    pub label: Entity<InputState>,
    pub secret: Entity<InputState>,
    pub error: Option<String>,
}

pub struct TransformForm {
    pub source: Option<PathBuf>,
    pub recover: bool,
    pub format: &'static str,
    pub source_password: Entity<InputState>,
    pub output_password: Entity<InputState>,
}

impl MainView {
    pub fn tool_text(&self, key: &str) -> &'static str {
        crate::text::tr(&self.settings.language, key)
    }

    fn recovery_issue_text(&self, issue: &str) -> String {
        let key = if issue.starts_with("ZIP central directory unavailable") {
            "issue_directory"
        } else if issue.contains("cannot be recovered without directory") {
            "issue_local"
        } else if issue.contains("symbolic links cannot be preserved") {
            "issue_link"
        } else if issue.contains("unsafe path in archive") {
            "issue_path"
        } else if issue.contains("checksum") || issue.contains("CRC") || issue.contains("crc") {
            "issue_checksum"
        } else if issue.contains("truncated") {
            "issue_truncated"
        } else if issue.contains("damaged solid block") {
            "issue_solid"
        } else if issue.contains("duplicate or conflicting") || issue.contains("parent directory") {
            "issue_conflict"
        } else {
            return issue.to_string();
        };
        if key == "issue_directory" {
            return self.tool_text(key).into();
        }
        let label = issue.split_once(": ").map_or("", |(label, _)| label);
        format!("{label}: {}", self.tool_text(key))
    }

    pub fn pick_favorite(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(self.tool_text("add_folder").into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let valid = cx
                    .background_executor()
                    .spawn(async move {
                        paths
                            .into_iter()
                            .filter_map(|p| std::fs::canonicalize(p).ok())
                            .filter(|p| p.is_dir())
                            .collect::<Vec<_>>()
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    for path in valid {
                        this.settings.hidden_locations.retain(|p| p != &path);
                        if !this.settings.favorites.contains(&path) {
                            this.settings.favorites.push(path);
                        }
                    }
                    this.save_settings(cx);
                    this.refresh_locations(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn remove_favorite(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        self.settings.hide_location(path);
        self.save_settings(cx);
        self.locations.list.items.retain(|item| item.path != path);
        if self.locations.list.selected.as_deref() == Some(path) {
            self.locations.list.selected = None;
        }
        self.refresh_locations(cx);
        cx.notify();
    }

    pub fn open_favorite(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let target = path.clone();
            let valid = cx
                .background_executor()
                .spawn(async move { std::fs::read_dir(target).is_ok() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if valid {
                    this.show_folder(path, cx);
                } else {
                    this.say(Notice::Error(this.tool_text("folder_error").into()), cx);
                }
            });
        })
        .detach();
    }

    pub fn refresh_password_book(&mut self, cx: &mut Context<Self>) {
        if self.tools.busy {
            return;
        }
        self.tools.generation += 1;
        let generation = self.tools.generation;
        self.tools.loading = true;
        self.tools.error = None;
        let vault = self.vault.clone();
        cx.spawn(async move |this, cx| {
            let labels = cx
                .background_executor()
                .spawn(async move { vault.try_labels() })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.tools.generation != generation {
                    return;
                }
                this.tools.loading = false;
                match labels {
                    Ok(mut labels) => {
                        labels.sort();
                        this.tools.labels = labels;
                    }
                    Err(_) => {
                        this.tools.labels.clear();
                        this.tools.error = Some(this.tool_text("vault_error").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn edit_password(
        &mut self,
        original: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tools.busy {
            return;
        }
        let label = cx.new(|cx| {
            InputState::new(window, cx).default_value(original.clone().unwrap_or_default())
        });
        let secret = cx.new(|cx| InputState::new(window, cx).masked(true));
        self.overlay = Some(Overlay::PasswordEdit(PasswordForm {
            original: original.clone(),
            label,
            secret: secret.clone(),
            error: None,
        }));
        if let Some(name) = original {
            self.tools.busy = true;
            let vault = self.vault.clone();
            cx.spawn_in(window, async move |this, cx| {
                let pw = cx
                    .background_executor()
                    .spawn(async move { vault.try_get(&name).map(|s| s.map(Zeroizing::new)) })
                    .await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.tools.busy = false;
                    match pw {
                        Ok(Some(value)) => {
                            secret.update(cx, |s, cx| s.set_value(value.as_str(), window, cx))
                        }
                        _ => {
                            let message = this.tool_text("vault_error").to_string();
                            if let Some(Overlay::PasswordEdit(f)) = &mut this.overlay {
                                f.error = Some(message);
                            }
                        }
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    pub fn save_password(&mut self, cx: &mut Context<Self>) {
        if self.tools.busy {
            return;
        }
        let Some(Overlay::PasswordEdit(form)) = &self.overlay else {
            return;
        };
        let label = form.label.read(cx).value().trim().to_string();
        let secret = Zeroizing::new(form.secret.read(cx).value().to_string());
        let creating = form.original.is_none();
        if label.is_empty()
            || label == "__labels__"
            || label.contains(['/', '\\'])
            || label.chars().any(char::is_control)
            || label.len() > 1024
            || secret.is_empty()
        {
            let message = self.tool_text("password_required").to_string();
            if let Some(Overlay::PasswordEdit(f)) = &mut self.overlay {
                f.error = Some(message);
            }
            cx.notify();
            return;
        }
        self.tools.busy = true;
        let vault = self.vault.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    if creating && vault.try_labels()?.contains(&label) {
                        return Ok(false);
                    }
                    vault.set(&label, &secret)?;
                    Ok::<_, rz_core::Error>(true)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tools.busy = false;
                match result {
                    Ok(true) => {
                        this.close_overlay(cx);
                        this.refresh_password_book(cx);
                    }
                    other => {
                        let key = if matches!(other, Ok(false)) {
                            "duplicate_label"
                        } else {
                            "vault_error"
                        };
                        let message = this.tool_text(key).to_string();
                        if let Some(Overlay::PasswordEdit(f)) = &mut this.overlay {
                            f.error = Some(message);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn delete_saved_password(&mut self, cx: &mut Context<Self>) {
        if self.tools.busy {
            return;
        }
        let Some(Overlay::PasswordDelete(label)) = &self.overlay else {
            return;
        };
        let (vault, label) = (self.vault.clone(), label.clone());
        self.tools.busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { vault.delete(&label) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tools.busy = false;
                match result {
                    Ok(()) => {
                        this.close_overlay(cx);
                        this.refresh_password_book(cx);
                    }
                    Err(_) => {
                        this.tools.error = Some(this.tool_text("vault_error").into());
                        this.say(Notice::Error(this.tool_text("vault_error").into()), cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn open_transform(&mut self, recover: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.busy {
            return;
        }
        let source = self.current().map(|a| a.path.clone());
        let password = self
            .current()
            .and_then(|a| a.password.clone())
            .unwrap_or_default();
        let source_password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .default_value(password)
        });
        let output_password = cx.new(|cx| InputState::new(window, cx).masked(true));
        cx.subscribe(
            &output_password,
            |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
        )
        .detach();
        self.overlay = Some(Overlay::Transform(TransformForm {
            source,
            recover,
            format: "zip",
            source_password,
            output_password,
        }));
        cx.notify();
    }

    pub fn pick_transform_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tool_text("choose_source").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(Overlay::Transform(f)) = &mut this.overlay {
                        f.source = paths.into_iter().next();
                        f.source_password
                            .update(cx, |s, cx| s.set_value("", window, cx));
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn submit_transform(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Transform(f)) = &self.overlay else {
            return;
        };
        let Some(source) = f.source.clone() else {
            return;
        };
        if self.tools.busy {
            return;
        }
        let recover = f.recover;
        let Some(format) = OutputFormat::from_path(&PathBuf::from(format!("output.{}", f.format)))
        else {
            return;
        };
        let input_pw = Zeroizing::new(f.source_password.read(cx).value().to_string());
        let output_pw = Zeroizing::new(f.output_password.read(cx).value().to_string());
        let opts = TransformOptions {
            format,
            create: self.settings.create_options(
                (format.supports_password() && !output_pw.is_empty())
                    .then(|| output_pw.to_string()),
            ),
            limits: self
                .settings
                .extract_options((!input_pw.is_empty()).then(|| input_pw.to_string())),
            encoding: self.settings.forced_encoding().map(str::to_string),
        };
        let name = format!(
            "{}-{}.{}",
            crate::recent::archive_stem(&source),
            if recover { "recovered" } else { "converted" },
            f.format
        );
        let directory = source
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let rx = cx.prompt_for_new_path(&directory, Some(&name));
        self.tools.busy = true;
        cx.spawn(async move |this, cx| {
            let target = rx.await;
            let _ = this.update(cx, |this, cx| {
                this.tools.busy = false;
                if let Ok(Ok(Some(output))) = target {
                    this.overlay = None;
                    this.tasks.submit(JobSpec::Transform {
                        archive: source,
                        // The system dialog may have confirmed replacing; both are kept instead.
                        output: rz_engine::unused_path(&output),
                        opts,
                        recover,
                    });
                    this.refresh_tasks(cx);
                    this.ensure_polling(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn render_tools_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        h_flex()
            .h(px(46.))
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(p.line)
            .child(
                Button::new("tool-add")
                    .primary()
                    .icon(IconName::Plus)
                    .label(self.tool_text("add_password"))
                    .disabled(self.tools.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_password(None, window, cx);
                    })),
            )
            .child(
                Button::new("tool-refresh")
                    .outline()
                    .label(self.tool_text("refresh"))
                    .disabled(self.tools.busy || self.tools.loading)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh_password_book(cx))),
            )
            .into_any_element()
    }

    pub fn render_tools_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let count = self.tools.labels.len();
        let entity = cx.entity();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_2()
            .p_3()
            .child(
                div()
                    .text_color(p.fg2)
                    .child(self.tool_text("password_hint")),
            )
            .when(!self.vault.is_persistent(), |d| {
                d.child(
                    div()
                        .text_color(p.warn)
                        .child(self.tool_text("memory_only")),
                )
            })
            .when_some(self.tools.error.clone(), |d, e| {
                d.child(div().text_color(p.danger).child(e))
            })
            .when(self.tools.loading, |d| {
                d.child(self.tool_text("folder_busy"))
            })
            .when(count == 0 && !self.tools.loading, |d| {
                d.child(self.tool_text("empty_passwords"))
            })
            .child(
                uniform_list("tool-list", count, move |range, _, cx| {
                    entity.update(cx, |this, cx| {
                        range
                            .map(|ix| this.render_tool_row(ix, cx))
                            .collect::<Vec<_>>()
                    })
                })
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn render_tool_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let Some(name) = self.tools.labels.get(ix).cloned() else {
            return div().h(px(38.)).into_any_element();
        };
        let target = name.clone();
        let remove = name.clone();
        h_flex()
            .w_full()
            .h(px(38.))
            .gap_2()
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(cx.theme().mono_font_family.clone())
                    .child(name),
            )
            .child(
                Button::new(("tool-open", ix))
                    .outline()
                    .small()
                    .label(self.tool_text("edit"))
                    .disabled(self.tools.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_password(Some(target.clone()), window, cx);
                    })),
            )
            .child(
                Button::new(("tool-remove", ix))
                    .ghost()
                    .small()
                    .text_color(p.danger)
                    .label(self.tool_text("delete"))
                    .disabled(self.tools.busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.overlay = Some(Overlay::PasswordDelete(remove.clone()));
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub fn password_edit_card(&self, f: &PasswordForm, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::KeyRound),
            title: self.tool_text("passwords").into(),
            lead: None,
            body: Some(
                v_flex()
                    .gap_2()
                    .child(self.tool_text("label"))
                    .child(Input::new(&f.label).disabled(f.original.is_some() || self.tools.busy))
                    .child(self.tool_text("secret"))
                    .child(
                        Input::new(&f.secret)
                            .mask_toggle()
                            .disabled(self.tools.busy),
                    )
                    .when_some(f.error.clone(), |d, e| {
                        d.child(div().text_color(p.danger).child(e))
                    })
                    .into_any_element(),
            ),
            buttons: vec![
                self.tool_cancel(cx),
                Button::new("password-save")
                    .primary()
                    .label(self.tool_text(if self.tools.busy { "busy" } else { "save" }))
                    .disabled(self.tools.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_password(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    pub fn password_delete_card(&self, label: &str, cx: &mut Context<Self>) -> AnyElement {
        Dialog {
            tone: Tone::Danger,
            icon: Icon::new(gpui_kit::assets::IconName::Trash),
            title: self.tool_text("confirm_delete").into(),
            lead: Some(self.tool_text("delete_hint").into()),
            body: Some(div().child(label.to_string()).into_any_element()),
            buttons: vec![
                self.tool_cancel(cx),
                Button::new("password-delete")
                    .primary()
                    .label(self.tool_text("delete"))
                    .disabled(self.tools.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.delete_saved_password(cx)))
                    .into_any_element(),
            ],
        }
        .render(&palette(cx))
    }

    fn tool_cancel(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("tool-cancel")
            .outline()
            .label(self.tool_text("cancel"))
            .disabled(self.tools.busy)
            .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
            .into_any_element()
    }

    pub fn transform_card(&self, f: &TransformForm, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let body = v_flex()
            .gap_2()
            .child(self.tool_text("source"))
            .when_some(f.source.clone(), |d, path| {
                d.child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                )
            })
            .child(
                Button::new("transform-source")
                    .outline()
                    .label(self.tool_text("choose_source"))
                    .disabled(self.tools.busy)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.pick_transform_source(window, cx)),
                    ),
            )
            .child(self.tool_text("source_password"))
            .child(
                Input::new(&f.source_password)
                    .mask_toggle()
                    .disabled(self.tools.busy),
            )
            .child(self.tool_text("format"))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(FORMATS.iter().enumerate().map(|(ix, &format)| {
                        Button::new(("transform-format", ix))
                            .outline()
                            .small()
                            .label(format)
                            .selected(f.format == format)
                            .disabled(self.tools.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(Overlay::Transform(f)) = &mut this.overlay {
                                    f.format = format;
                                }
                                cx.notify();
                            }))
                    })),
            )
            .when(matches!(f.format, "zip" | "7z"), |d| {
                d.child(self.tool_text("output_password")).child(
                    Input::new(&f.output_password)
                        .mask_toggle()
                        .disabled(self.tools.busy),
                )
            })
            .when(
                !matches!(f.format, "zip" | "7z") || f.output_password.read(cx).value().is_empty(),
                |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(p.warn)
                            .child(self.tool_text("unencrypted")),
                    )
                },
            );
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::FileArchive),
            title: self
                .tool_text(if f.recover { "repair" } else { "convert" })
                .into(),
            lead: Some(
                self.tool_text(if f.recover {
                    "repair_hint"
                } else {
                    "convert_hint"
                })
                .into(),
            ),
            body: Some(body.into_any_element()),
            buttons: vec![
                self.tool_cancel(cx),
                Button::new("transform-go")
                    .primary()
                    .label(self.tool_text(if f.recover {
                        "start_repair"
                    } else {
                        "start_convert"
                    }))
                    .disabled(self.tools.busy || f.source.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.submit_transform(cx)))
                    .into_any_element(),
            ],
        }
        .render(&p)
    }

    pub fn recovery_card(&self, id: TaskId, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let summary = self
            .task_view
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.summary.as_ref());
        let report = summary.and_then(|s| s.recovery.as_ref());
        let title = report.map(|r| {
            self.tool_text("report_counts")
                .replace("{files}", &r.files.to_string())
                .replace("{skipped}", &r.skipped.to_string())
        });
        let body = v_flex().gap_2().when_some(report, |d, r| {
            d.child(self.tool_text(if r.issues.is_empty() {
                "report_complete"
            } else {
                "report_warnings"
            }))
            .children(
                r.issues
                    .iter()
                    .map(|s| div().text_size(px(12.)).child(self.recovery_issue_text(s))),
            )
            .when(r.issues.len() == 1000, |d| {
                d.child(self.tool_text("report_truncated"))
            })
        });
        let mut buttons = vec![
            Button::new("report-close")
                .outline()
                .label(self.tool_text("close"))
                .on_click(cx.listener(|this, _, _, cx| this.close_overlay(cx)))
                .into_any_element(),
        ];
        if let Some(summary) = summary {
            let output = summary.output.clone();
            buttons.push(
                Button::new("report-output")
                    .primary()
                    .label(self.tool_text("open_folder"))
                    .on_click(move |_, _, cx| cx.reveal_path(&output))
                    .into_any_element(),
            );
        }
        Dialog {
            tone: Tone::Neutral,
            icon: Icon::new(gpui_kit::assets::IconName::FileArchive),
            title: self.tool_text("report").into(),
            lead: title,
            body: Some(body.into_any_element()),
            buttons,
        }
        .render(&p)
    }
}
