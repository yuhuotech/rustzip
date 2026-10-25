//! Common locations and folder contents share the same compact browser and view preference.

use crate::brand::palette;
pub use crate::location_state::LocationsState;
use crate::recent::{self, RecentItem, RecentList, RecentSort};
use crate::view::{MainView, Page};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::*;
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::browse::{RowKind, badge_label, format_size};
use rz_app::settings::FolderView;
use std::path::PathBuf;

/// Metadata is read off the window thread; directories are never recursively sized.
pub fn scan(settings: &rz_app::Settings) -> Vec<RecentItem> {
    rz_app::locations::collect(settings)
        .into_iter()
        .map(|location| {
            let meta = std::fs::metadata(&location.path)
                .ok()
                .filter(|m| m.is_dir());
            RecentItem {
                name: location.system.map_or_else(
                    || {
                        location
                            .path
                            .file_name()
                            .unwrap_or(location.path.as_os_str())
                            .to_string_lossy()
                            .into_owned()
                    },
                    |kind| crate::text::tr(&settings.language, kind.key()).into(),
                ),
                folder: location.path.to_string_lossy().into_owned(),
                created: meta.as_ref().and_then(|m| recent::unix(m.created())),
                modified: meta.as_ref().and_then(|m| recent::unix(m.modified())),
                exists: meta.is_some(),
                is_dir: true,
                size: None,
                path: location.path,
            }
        })
        .collect()
}

impl MainView {
    pub fn refresh_locations(&mut self, cx: &mut Context<Self>) {
        let generation = self.locations.begin_scan();
        let settings = self.settings.clone();
        cx.spawn(async move |this, cx| {
            let items = cx
                .background_executor()
                .spawn(async move { scan(&settings) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.locations.apply_scan(generation, items) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn set_folder_view(&mut self, view: FolderView, cx: &mut Context<Self>) {
        self.settings.folder_view = view;
        self.locations.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.save_settings(cx);
        cx.notify();
    }

    pub(crate) fn reset_directory_scroll(&self) {
        self.locations.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }

    pub fn render_folder_view_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        h_flex()
            .id("folder-view-switch")
            .flex_none()
            .px_3()
            .gap_1()
            .child(
                Button::new("view-list")
                    .outline()
                    .small()
                    .icon(gpui_kit::assets::IconName::List)
                    .label(self.tool_text("view_list"))
                    .tooltip(self.tool_text("view_list"))
                    .selected(self.settings.folder_view == FolderView::List)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_folder_view(FolderView::List, cx)),
                    ),
            )
            .child(
                Button::new("view-icons")
                    .outline()
                    .small()
                    .icon(gpui_kit::assets::IconName::LayoutGrid)
                    .label(self.tool_text("view_icons"))
                    .tooltip(self.tool_text("view_icons"))
                    .selected(self.settings.folder_view == FolderView::Icons)
                    .on_click(
                        cx.listener(|this, _, _, cx| this.set_folder_view(FolderView::Icons, cx)),
                    ),
            )
            .text_color(p.fg2)
            .into_any_element()
    }

    pub fn render_locations_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        h_flex()
            .id("locations-toolbar")
            .h(px(46.))
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .px_3()
                    .gap_1()
                    .child(
                        Button::new("tool-add")
                            .outline()
                            .icon(IconName::Plus)
                            .label(self.tool_text("add_location"))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_favorite(cx))),
                    )
                    .child(
                        Button::new("locations-refresh")
                            .ghost()
                            .icon(IconName::RotateCw)
                            .tooltip(self.tool_text("refresh"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_locations(cx))),
                    ),
            )
            .child(self.render_folder_view_switch(cx))
            .into_any_element()
    }

    fn directory_items(&self) -> &RecentList {
        if self.page == Page::Favorites {
            &self.locations.list
        } else {
            &self.files
        }
    }

    fn directory_items_mut(&mut self) -> &mut RecentList {
        if self.page == Page::Favorites {
            &mut self.locations.list
        } else {
            &mut self.files
        }
    }

    pub fn sort_directory(&mut self, sort: RecentSort, cx: &mut Context<Self>) {
        if self.page == Page::Favorites && !self.locations.sorted {
            self.locations.list.sort = sort;
            self.locations.list.ascending = matches!(sort, RecentSort::Name | RecentSort::Folder);
            let items = std::mem::take(&mut self.locations.list.items);
            self.locations.list.set_items(items);
            self.locations.sorted = true;
        } else {
            self.directory_items_mut().sort_by(sort);
        }
        cx.notify();
    }

    pub fn select_directory_item(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.directory_items_mut().selected = Some(path);
        self.notice = None;
        cx.notify();
    }

    fn open_directory_item(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.page == Page::Favorites {
            self.open_favorite(path, cx);
        } else {
            self.open_listed(path, is_dir, window, cx);
        }
    }

    fn directory_key(
        &mut self,
        key: &str,
        columns: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if key == "enter" {
            if let Some(item) = self.directory_items().selected_item().cloned() {
                self.open_directory_item(item.path, item.is_dir, window, cx);
            }
            return;
        }
        if key == "backspace" {
            if self.page == Page::Folder {
                self.folder_up(cx);
            }
            return;
        }
        let list = self.directory_items();
        let len = list.items.len();
        if len == 0 {
            return;
        }
        let selected = list
            .selected
            .as_ref()
            .and_then(|p| list.items.iter().position(|i| &i.path == p));
        let stride = if self.settings.folder_view == FolderView::Icons {
            columns
        } else {
            1
        };
        let next = match key {
            "down" => selected.map_or(0, |i| i.saturating_add(stride).min(len - 1)),
            "up" => selected.map_or(0, |i| i.saturating_sub(stride)),
            "right" => selected.map_or(0, |i| (i + 1).min(len - 1)),
            "left" => selected.map_or(0, |i| i.saturating_sub(1)),
            _ => return,
        };
        let path = list.items[next].path.clone();
        self.select_directory_item(path, cx);
        let slot = next + usize::from(self.page == Page::Folder);
        self.locations.scroll.scroll_to_item(
            if self.settings.folder_view == FolderView::Icons {
                slot / columns
            } else {
                slot
            },
            ScrollStrategy::Center,
        );
    }

    pub fn render_locations_browser(
        &self,
        window_width: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = palette(cx);
        let overview = self.page == Page::Favorites;
        let width: f32 =
            (window_width - px(if window_width >= px(560.) { 168. } else { 0. })).into();
        let list = self.directory_items();
        let icons = self.settings.folder_view == FolderView::Icons;
        let up = !overview;
        let columns = ((width - 24.).max(112.) / 112.).floor() as usize;
        let count = list.items.len() + usize::from(up);
        let rows = if icons {
            count.div_ceil(columns)
        } else {
            count
        };
        let entity = cx.entity();
        v_flex()
            .id("locations-browser")
            .track_focus(&self.directory_focus)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                {
                    return;
                }
                let key = event.keystroke.key.as_str();
                if matches!(
                    key,
                    "enter" | "backspace" | "up" | "down" | "left" | "right"
                ) {
                    this.directory_key(key, columns, window, cx);
                    cx.stop_propagation();
                }
            }))
            .when(!overview, |d| {
                d.child(
                    h_flex()
                        .h(px(36.))
                        .flex_none()
                        .px_3()
                        .gap_2()
                        .border_b_1()
                        .border_color(p.line)
                        .child(
                            Button::new("locations-back")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowLeft)
                                .label(self.tool_text("favorites"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_page(Page::Favorites, cx)
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(p.fg2)
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(
                                    self.folder
                                        .as_ref()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                ),
                        ),
                )
            })
            .when(!icons, |d| {
                d.child(self.directory_header(overview, width, cx))
            })
            .when(overview && self.locations.loading, |d| {
                d.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_color(p.fg3)
                        .child(self.tool_text("folder_busy")),
                )
            })
            .when(
                list.items.is_empty() && !(overview && self.locations.loading),
                |d| {
                    d.child(
                        div()
                            .p_4()
                            .text_color(p.fg3)
                            .child(self.tool_text(if overview {
                                "locations_empty"
                            } else {
                                "directory_empty"
                            })),
                    )
                },
            )
            .child(
                uniform_list(
                    if icons {
                        "directory-grid"
                    } else {
                        "directory-list"
                    },
                    rows,
                    move |range, _, cx| {
                        entity.update(cx, |this, cx| {
                            range
                                .map(|row| {
                                    if icons {
                                        h_flex()
                                            .h(px(104.))
                                            .items_start()
                                            .px_3()
                                            .children((0..columns).filter_map(|column| {
                                                let ix = row * columns + column;
                                                (ix < count).then(|| {
                                                    this.directory_cell(ix, up, true, width, cx)
                                                })
                                            }))
                                            .into_any_element()
                                    } else {
                                        this.directory_cell(row, up, false, width, cx)
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                    },
                )
                .track_scroll(&self.locations.scroll)
                .flex_1()
                .min_h_0(),
            )
            .into_any_element()
    }

    fn directory_header(&self, overview: bool, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        let title = |id, key, sort, right| {
            let on = (self.page != Page::Favorites || self.locations.sorted)
                && self.directory_items().sort == sort;
            h_flex()
                .id(id)
                .h_full()
                .gap_1()
                .cursor_pointer()
                .text_color(if on { p.fg2 } else { p.fg3 })
                .when(right, |d| d.justify_end())
                .on_click(cx.listener(move |this, _, _, cx| this.sort_directory(sort, cx)))
                .child(self.tool_text(key))
                .when(on, |d| {
                    d.child(
                        Icon::new(if self.directory_items().ascending {
                            IconName::ArrowUp
                        } else {
                            IconName::ArrowDown
                        })
                        .size_3(),
                    )
                })
        };
        h_flex()
            .h(px(28.))
            .flex_none()
            .px_2()
            .gap_2()
            .text_size(px(12.))
            .border_b_1()
            .border_color(p.line)
            .child(
                title("directory-name", "col_name", RecentSort::Name, false)
                    .flex_1()
                    .min_w_0(),
            )
            .child(title("directory-size", "col_size", RecentSort::Size, true).w(px(84.)))
            .when(overview && width >= 620., |d| {
                d.child(
                    title("directory-path", "col_path", RecentSort::Folder, false)
                        .flex_1()
                        .min_w_0(),
                )
            })
            .child(
                title(
                    "directory-modified",
                    "col_modified",
                    RecentSort::Modified,
                    false,
                )
                .w(px(136.)),
            )
            .into_any_element()
    }

    fn directory_cell(
        &self,
        ix: usize,
        up: bool,
        icons: bool,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if up && ix == 0 {
            return self.directory_up_cell(icons, cx);
        }
        let index = ix - usize::from(up);
        let Some(item) = self.directory_items().items.get(index) else {
            return div().h(px(28.)).into_any_element();
        };
        let p = palette(cx);
        let on = self.directory_items().selected.as_ref() == Some(&item.path);
        let overview = self.page == Page::Favorites;
        let path = item.path.clone();
        let select_on_right = path.clone();
        let kind = if item.is_dir {
            RowKind::Dir
        } else {
            RowKind::File
        };
        let mono = cx.theme().mono_font_family.clone();
        let element = div()
            .id((
                if icons {
                    "directory-icon"
                } else {
                    "directory-row"
                },
                index,
            ))
            .aria_label(item.name.clone())
            .cursor_pointer()
            .text_color(if item.exists { p.fg } else { p.fg3 })
            .when(on, |d| d.bg(p.accent_soft))
            .when(!on, |d| d.hover(|d| d.bg(p.hover)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.directory_focus.focus(window, cx);
                this.select_directory_item(path.clone(), cx);
                if event.click_count() >= 2 {
                    this.open_directory_item(path.clone(), kind == RowKind::Dir, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    this.directory_focus.focus(window, cx);
                    this.select_directory_item(select_on_right.clone(), cx);
                }),
            );
        let element = if icons {
            element
                .w(px(112.))
                .h(px(104.))
                .p_2()
                .rounded(px(8.))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(if item.is_dir {
                    Icon::new(IconName::Folder)
                        .size(px(40.))
                        .text_color(p.accent)
                        .into_any_element()
                } else {
                    v_flex()
                        .w(px(40.))
                        .h(px(40.))
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .bg(p.hover)
                        .child(
                            div()
                                .text_size(px(10.))
                                .font_family(mono)
                                .child(badge_label(&item.name, kind)),
                        )
                        .into_any_element()
                })
                .child(
                    div()
                        .w_full()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_center()
                        .text_size(px(12.))
                        .child(item.name.clone()),
                )
                .when(!item.exists, |d| {
                    d.child(
                        div()
                            .text_size(px(10.))
                            .text_color(p.warn)
                            .child(self.tool_text("location_missing")),
                    )
                })
        } else {
            element
                .w_full()
                .h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(p.line)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .w(px(32.))
                                .h(px(18.))
                                .flex_none()
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(if item.is_dir { p.accent_soft } else { p.hover })
                                .text_color(if item.is_dir { p.accent } else { p.fg2 })
                                .font_family(mono.clone())
                                .text_size(px(9.))
                                .child(badge_label(&item.name, kind)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(item.name.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(84.))
                        .text_right()
                        .font_family(mono.clone())
                        .text_size(px(12.))
                        .text_color(p.fg2)
                        .child(if !item.exists {
                            self.tool_text("location_missing").into()
                        } else {
                            item.size.map(format_size).unwrap_or_else(|| "—".into())
                        }),
                )
                .when(overview && width >= 620., |d| {
                    d.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_family(mono.clone())
                            .text_size(px(12.))
                            .text_color(p.fg3)
                            .child(item.folder.clone()),
                    )
                })
                .child(
                    div()
                        .w(px(136.))
                        .font_family(mono)
                        .text_size(px(12.))
                        .text_color(p.fg2)
                        .child(
                            item.modified
                                .map(crate::local_time)
                                .unwrap_or_else(|| "—".into()),
                        ),
                )
        };
        let view = cx.entity();
        let remove = item.path.clone();
        let open = item.path.clone();
        let exists = item.exists;
        let is_dir = item.is_dir;
        let open_label = self.tool_text("open");
        let remove_label = self.tool_text("remove_location");
        let path_label = item.path.to_string_lossy().into_owned();
        element
            .test_support()
            .context_menu(move |menu, _, _| {
                let menu = menu.label(path_label.clone()).separator();
                let view_open = view.clone();
                let target = open.clone();
                let menu = menu.item(PopupMenuItem::new(open_label).disabled(!exists).on_click(
                    move |_, window, cx| {
                        view_open.update(cx, |this, cx| {
                            this.open_directory_item(target.clone(), is_dir, window, cx)
                        });
                    },
                ));
                if overview {
                    let view_remove = view.clone();
                    let path = remove.clone();
                    menu.separator()
                        .item(PopupMenuItem::new(remove_label).on_click(move |_, _, cx| {
                            view_remove.update(cx, |this, cx| this.remove_favorite(&path, cx));
                        }))
                } else {
                    menu
                }
            })
            .into_any_element()
    }

    fn directory_up_cell(&self, icons: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = palette(cx);
        div()
            .id("folder-up")
            .cursor_pointer()
            .text_color(p.fg2)
            .when(icons, |d| {
                d.w(px(112.))
                    .h(px(104.))
                    .p_2()
                    .rounded(px(8.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
            })
            .when(!icons, |d| {
                d.w_full()
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(p.line)
            })
            .hover(|d| d.bg(p.hover))
            .child(
                Icon::new(IconName::Folder)
                    .size(px(if icons { 40. } else { 16. }))
                    .text_color(p.accent),
            )
            .child("../")
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.folder_up(cx);
                }
            }))
            .test_support()
            .into_any_element()
    }
}
