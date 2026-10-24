//! The file list: a virtualised table whose rows come from the archive tree.

use crate::brand::palette;
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rz_app::browse::{
    ArchiveTree, Badge, Row, RowKind, SortKey, badge, badge_label, format_size, format_time,
};
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
}

impl Default for FileRows {
    fn default() -> Self {
        let mut size = Column::new("size", "大小").width(px(110.)).sortable();
        size.align = TextAlign::Right;
        let mut packed = Column::new("packed", "压缩后").width(px(110.));
        packed.align = TextAlign::Right;
        Self {
            tree: None,
            cwd: String::new(),
            filter: String::new(),
            sort: SortKey::Name,
            ascending: true,
            rows: Vec::new(),
            columns: vec![
                Column::new("name", "名称").width(px(420.)).sortable(),
                size,
                packed,
                Column::new("time", "修改时间").width(px(170.)).sortable(),
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
        self.reload();
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

/// The coloured badge in front of a name.
pub fn badge_chip(row: &Row, cx: &App) -> Div {
    let p = palette(cx);
    let (bg, fg) = match badge(&row.name, row.kind) {
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
    };
    div()
        .w(px(30.))
        .h(px(22.))
        .rounded(px(6.))
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
                let text = row.packed.map_or_else(|| "—".into(), format_size);
                mono(text, p.fg2, cx)
            }
            _ => {
                let text = row.modified.map_or_else(|| "—".into(), format_time);
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
