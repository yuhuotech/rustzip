//! The file list: a virtualised table whose rows come from the archive tree.

use crate::brand::palette;
use crate::{
    RowAddHere, RowCopyPath, RowDelete, RowExtractHere, RowExtractNamed, RowExtractTo, RowOpen,
    RowRename,
};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::browse::{ArchiveTree, Badge, Row, RowKind, SortKey, badge, badge_label, format_size};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const COL_NAME: usize = 0;
pub const COL_SIZE: usize = 1;
pub const COL_PACKED: usize = 2;
pub const COL_TIME: usize = 3;

pub struct FileRows {
    tree: Option<Arc<ArchiveTree>>,
    cwd: String,
    filter: String,
    sort: SortKey,
    ascending: bool,
    rows: Vec<Row>,
    columns: Vec<Column>,
    /// Paths picked with ⌘/Ctrl-click or Shift-click, besides the table's own row.
    marked: BTreeSet<String>,
    /// Whether the archive can be changed, and the name for "extract to a folder of the
    /// same name"; both only shape the right-click menu.
    editable: bool,
    stem: String,
}

impl Default for FileRows {
    fn default() -> Self {
        let mut size = Column::new("size", "大小").width(px(84.)).sortable();
        size.align = TextAlign::Right;
        let mut packed = Column::new("packed", "压缩后").width(px(84.));
        packed.align = TextAlign::Right;
        Self {
            tree: None,
            cwd: String::new(),
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
            rows: Vec::new(),
            marked: BTreeSet::new(),
            editable: false,
            stem: String::new(),
            columns: vec![
                Column::new("name", "名称").width(px(340.)).sortable(),
                size,
                packed,
                Column::new("time", "修改时间").width(px(128.)).sortable(),
            ],
        }
    }
}

impl FileRows {
    /// Points the list at a folder of an archive (or at nothing).
    pub fn show(&mut self, tree: Option<Arc<ArchiveTree>>, cwd: &str, filter: &str) {
        self.tree = tree;
        self.cwd = cwd.to_string();
        self.filter = filter.to_string();
        self.marked.clear();
        self.reload();
    }

    /// What the right-click menu needs to know about the archive.
    pub fn set_menu_context(&mut self, editable: bool, stem: &str) {
        self.editable = editable;
        self.stem = stem.to_string();
    }

    /// Picks every row that is listed.
    pub fn mark_all(&mut self) {
        self.marked = self.rows.iter().map(|r| r.path.clone()).collect();
    }

    /// Everything the user has picked, in path order.
    pub fn marked(&self) -> Vec<String> {
        self.marked.iter().cloned().collect()
    }

    /// A plain click: exactly this row is picked.
    pub fn mark_only(&mut self, path: &str) {
        self.marked.clear();
        self.marked.insert(path.to_string());
    }

    pub fn toggle(&mut self, ix: usize) {
        if let Some(r) = self.rows.get(ix)
            && !self.marked.remove(&r.path)
        {
            self.marked.insert(r.path.clone());
        }
    }

    /// Shift-click: everything between the anchor row and this one.
    pub fn mark_range(&mut self, anchor: usize, ix: usize) {
        let (lo, hi) = (anchor.min(ix), anchor.max(ix));
        self.marked = self
            .rows
            .iter()
            .skip(lo)
            .take(hi - lo + 1)
            .map(|r| r.path.clone())
            .collect();
    }

    fn reload(&mut self) {
        self.rows = match &self.tree {
            Some(t) => t.list(&self.cwd, self.sort, self.ascending, &self.filter),
            None => Vec::new(),
        };
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row(&self, ix: usize) -> Option<&Row> {
        self.rows.get(ix)
    }

    pub fn position_of(&self, path: &str) -> Option<usize> {
        self.rows.iter().position(|r| r.path == path)
    }
}

fn sort_key_for(col_ix: usize) -> SortKey {
    match col_ix {
        COL_SIZE => SortKey::Size,
        COL_TIME => SortKey::Modified,
        _ => SortKey::Name,
    }
}

/// Background and text colours of a file's badge.
pub fn badge_colors(row: &Row, cx: &App) -> (Hsla, Hsla) {
    let p = palette(cx);
    match badge(&row.name, row.kind) {
        Badge::Folder => (p.accent_soft, p.accent),
        Badge::Image => (
            hsla(217. / 360., 0.9, 0.6, 0.16),
            hsla(217. / 360., 0.9, 0.55, 1.),
        ),
        Badge::Document => (hsla(38. / 360., 0.9, 0.5, 0.18), p.warn),
        Badge::Code => (
            hsla(262. / 360., 0.8, 0.6, 0.16),
            hsla(262. / 360., 0.7, 0.6, 1.),
        ),
        Badge::Archive => (p.accent_soft, p.accent),
        Badge::Media => (
            hsla(330. / 360., 0.8, 0.6, 0.16),
            hsla(330. / 360., 0.7, 0.55, 1.),
        ),
        Badge::Other => (p.hover, p.fg2),
    }
}

/// The coloured badge in front of a name.
pub fn badge_chip(row: &Row, cx: &App) -> Div {
    let (bg, fg) = badge_colors(row, cx);
    div()
        .w(px(32.))
        .h(px(18.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .bg(bg)
        .text_color(fg)
        .text_size(px(9.))
        .child(badge_label(&row.name, row.kind))
}

impl TableDelegate for FileRows {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, ix: usize, _: &App) -> Column {
        self.columns[ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Column titles are labels, not content: a notch smaller than the rows.
        div()
            .size_full()
            .text_size(px(12.))
            .child(self.column(col_ix, cx).name)
    }

    /// The right-click menu. Like in a file manager, a right click on something that is
    /// not part of the selection first selects just that; a click inside the selection
    /// keeps the whole selection, so a menu entry acts on all of it.
    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        if let Some(row) = self.rows.get(row_ix).cloned()
            && !self.marked.contains(&row.path)
        {
            self.mark_only(&row.path);
            cx.emit(TableEvent::SelectRow(row_ix));
        }
        let picked: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| self.marked.contains(&r.path))
            .collect();
        let one = (picked.len() == 1).then(|| picked[0]);
        let editable = self.editable;
        let stem = if self.stem.is_empty() {
            "同名".to_string()
        } else {
            self.stem.clone()
        };
        let mut menu = menu;
        if picked.len() > 1 {
            menu = menu.label(format!("已选 {} 项", picked.len()));
        }
        // Opening is for one thing at a time.
        menu = match one.map(|r| r.kind) {
            Some(RowKind::Dir) => {
                menu.menu_with_icon("打开文件夹", IconName::FolderOpen, Box::new(RowOpen))
            }
            Some(RowKind::File) => {
                menu.menu_with_icon("打开", IconName::ExternalLink, Box::new(RowOpen))
            }
            _ => menu,
        };
        menu = menu
            .separator()
            .menu_with_icon("解压到…", IconName::ArrowDown, Box::new(RowExtractTo))
            .menu_with_icon(
                "解压到当前文件夹",
                IconName::Folder,
                Box::new(RowExtractHere),
            )
            .menu_with_icon(
                format!("解压到“{stem}”文件夹"),
                IconName::FolderOpen,
                Box::new(RowExtractNamed),
            )
            .separator()
            .menu_with_icon("复制路径", IconName::Copy, Box::new(RowCopyPath));
        // Changing the archive needs a format that can be rewritten.
        menu = menu.separator();
        if one.is_some_and(|r| r.kind == RowKind::Dir) {
            menu = menu.menu_with_icon_and_disabled(
                "添加文件到此文件夹…",
                IconName::Plus,
                Box::new(RowAddHere),
                !editable,
            );
        }
        menu.menu_with_icon_and_disabled(
            "重命名",
            IconName::Replace,
            Box::new(RowRename),
            !editable || one.is_none(),
        )
        .menu_with_icon_and_disabled(
            "删除",
            Icon::new(gpui_kit::assets::IconName::Trash),
            Box::new(RowDelete),
            !editable,
        )
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let p = palette(cx);
        let picked = self
            .rows
            .get(row_ix)
            .is_some_and(|r| self.marked.contains(&r.path));
        div()
            .id(("row", row_ix))
            .when(picked, |d| d.bg(p.accent_soft))
            .on_click(cx.listener(move |state, ev: &ClickEvent, _, cx| {
                let m = ev.modifiers();
                if m.secondary() {
                    state.delegate_mut().toggle(row_ix);
                } else if m.shift {
                    let anchor = state.selected_row().unwrap_or(row_ix);
                    state.delegate_mut().mark_range(anchor, row_ix);
                } else {
                    return;
                }
                cx.notify();
            }))
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = sort_key_for(col_ix);
        self.ascending = !matches!(sort, ColumnSort::Descending);
        self.reload();
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let p = palette(cx);
        match col_ix {
            COL_NAME => {
                let shown = if self.filter.is_empty() {
                    row.name.clone()
                } else {
                    // In search results the folder helps tell equal names apart.
                    row.path.clone()
                };
                h_flex()
                    .gap_2()
                    .items_center()
                    .text_size(px(13.))
                    .child(badge_chip(row, cx))
                    .child(div().overflow_hidden().text_ellipsis().child(shown))
                    .when(row.encrypted, |d| {
                        d.child(div().text_color(p.fg3).child("🔒"))
                    })
                    .into_any_element()
            }
            COL_SIZE => {
                let text = match row.kind {
                    RowKind::Dir => format!("{} 项", row.items),
                    _ => format_size(row.size),
                };
                mono(text, p.fg2, cx)
            }
            COL_PACKED => {
                // A folder has no packed size of its own.
                let text = match (row.kind, row.packed) {
                    (RowKind::File | RowKind::Link, Some(packed)) => format_size(packed),
                    _ => "—".into(),
                };
                mono(text, p.fg2, cx)
            }
            _ => {
                let text = row.modified.map_or_else(|| "—".into(), crate::local_time);
                mono(text, p.fg2, cx)
            }
        }
    }
}

fn mono(text: String, color: Hsla, cx: &App) -> AnyElement {
    div()
        .w_full()
        .text_right()
        .text_color(color)
        .text_size(px(12.))
        .font_family(cx.theme().mono_font_family.clone())
        .child(text)
        .into_any_element()
}
