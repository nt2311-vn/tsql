//! Whole-schema ERD canvas.
//!
//! Pipeline:
//!   1. Pick per-table (w, h) from current [`Zoom`] level.
//!   2. Call [`layered_layout`] to assign each table a slot in a
//!      virtual grid larger than the screen.
//!   3. Paint every card and every FK edge into that virtual grid.
//!   4. Slice the virtual grid by the [`Viewport`] offset and emit
//!      only the visible rows as ratatui `Line`s.

use std::collections::{HashMap, HashSet};

use ratatui::style::Color;
use ratatui::text::Line;
use tsqlx_db::{ColumnInfo, RelationshipEdge, TableInfo};

use crate::erd::layout::{layered_layout, NodePlacement};
use crate::erd::primitives::{draw_arrow, draw_card, grid_to_lines, put_text, Cell2};
use crate::erd::viewport::{Viewport, Zoom};
use crate::Theme;

/// Inputs to render the whole-schema canvas.
pub struct CanvasInput<'a> {
    pub tables: &'a [String],
    pub table_info: &'a HashMap<String, TableInfo>,
    pub edges: &'a [RelationshipEdge],
    pub selected: Option<&'a str>,
    pub viewport: &'a Viewport,
    pub view_w: u16,
    pub view_h: u16,
    pub theme: Theme,
}

/// Result of [`render_schema_canvas`]: lines ready to paste into a
/// `Paragraph`, plus the virtual canvas size (useful for clamping the
/// viewport after a zoom change or terminal resize).
pub struct CanvasOutput {
    pub lines: Vec<Line<'static>>,
    pub virtual_w: u16,
    pub virtual_h: u16,
    /// Where every card landed, in virtual coordinates. Reserved for
    /// the upcoming `c` recentre-on-selected key; surfaced now so
    /// callers don't need a follow-up layout pass to discover it.
    #[allow(dead_code)]
    pub placements: Vec<NodePlacement>,
}

/// Compute the (width, height) of a card at the requested zoom.
pub fn card_size(name: &str, info: Option<&TableInfo>, zoom: Zoom) -> (u16, u16) {
    let name_w = name.chars().count() as u16;
    match zoom {
        // Collapsed mode is the "fit the whole schema on screen"
        // density: truncate long names to 10 chars so a single very
        // wide table can't bloat the whole rank's column width.
        Zoom::Collapsed => (name_w.min(10).saturating_add(4).max(8), 3),
        Zoom::Compact => compact_size(name, info, name_w),
        Zoom::Full => full_size(name, info, name_w),
    }
}

fn compact_size(_name: &str, info: Option<&TableInfo>, name_w: u16) -> (u16, u16) {
    let Some(i) = info else {
        // Loading state — leave room for "(loading…)".
        return (name_w.saturating_add(4).max(16), 4);
    };
    let pk_set: HashSet<&str> = i
        .primary_key
        .as_ref()
        .map(|pk| pk.column_names.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let fk_set: HashSet<&str> = i
        .foreign_keys
        .iter()
        .flat_map(|fk| fk.column_names.iter().map(String::as_str))
        .collect();
    let rows: Vec<&ColumnInfo> = i
        .columns
        .iter()
        .filter(|c| pk_set.contains(c.name.as_str()) || fk_set.contains(c.name.as_str()))
        .collect();
    let widest = rows
        .iter()
        .map(|c| c.name.chars().count() + c.data_type.chars().count() + 5)
        .max()
        .unwrap_or(0) as u16;
    let w = name_w.saturating_add(4).max(widest).max(16);
    // header + rule + rows (at least 1).
    let h = 3u16.saturating_add(rows.len().max(1) as u16);
    (w, h)
}

fn full_size(_name: &str, info: Option<&TableInfo>, name_w: u16) -> (u16, u16) {
    let Some(i) = info else {
        return (name_w.saturating_add(4).max(16), 4);
    };
    let widest = i
        .columns
        .iter()
        .map(|c| c.name.chars().count() + c.data_type.chars().count() + 5)
        .max()
        .unwrap_or(0) as u16;
    let w = name_w.saturating_add(4).max(widest).max(16);
    let h = 3u16.saturating_add(i.columns.len().max(1) as u16);
    (w, h)
}

/// Render the whole-schema canvas. The returned `lines` cover exactly
/// `(view_w, view_h)` cells — slicing happens inside this function so
/// the caller can drop the result straight into a `Paragraph`.
pub fn render_schema_canvas(inp: CanvasInput<'_>) -> CanvasOutput {
    let th = inp.theme;
    let zoom = inp.viewport.zoom;
    // Gap sizing scales with zoom so Collapsed mode actually packs
    // tighter rather than just shrinking cards by a few rows. Compact
    // and Full both want breathing room for the FK column labels that
    // ride on the horizontal legs.
    let (h_gap, v_gap): (u16, u16) = match zoom {
        Zoom::Collapsed => (3, 0),
        Zoom::Compact => (5, 1),
        Zoom::Full => (6, 2),
    };

    // ── 1. Size every card ────────────────────────────────────────
    let sizes: HashMap<&str, (u16, u16)> = inp
        .tables
        .iter()
        .map(|t| {
            let info = inp.table_info.get(t);
            (t.as_str(), card_size(t, info, zoom))
        })
        .collect();

    // ── 2. Lay out ────────────────────────────────────────────────
    let layout_out = layered_layout(
        inp.tables,
        inp.edges,
        |n| sizes.get(n).copied().unwrap_or((12, 3)),
        h_gap,
        v_gap,
    );

    // ── 3. Paint the full virtual grid ────────────────────────────
    let virt_w = layout_out.virtual_w.max(1);
    let virt_h = layout_out.virtual_h.max(1);
    let mut grid: Vec<Vec<Cell2>> = vec![vec![Cell2::space(th); virt_w as usize]; virt_h as usize];

    let by_name: HashMap<&str, &NodePlacement> = layout_out
        .nodes
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    // ── 4. Paint edges FIRST so the card boxes below cleanly mask
    // any arrow leg that would otherwise carve through their bodies.
    // Lane allocation groups by DESTINATION card: each incoming edge
    // bends in the gutter immediately to the left of its target, with
    // per-edge lane offsets so multiple parents fanning into the same
    // child stagger instead of overlapping. Edges that span more than
    // one rank fly horizontally across intermediate ranks at the
    // source row — the mask-by-card-redraw step at (5) covers the
    // crossings cleanly.
    let mut edges_by_child: HashMap<&str, Vec<&RelationshipEdge>> = HashMap::new();
    for e in inp.edges {
        if e.from_table == e.to_table {
            continue; // self-FKs not visualised in v1
        }
        let Some(parent) = by_name.get(e.to_table.as_str()) else {
            continue;
        };
        let Some(child) = by_name.get(e.from_table.as_str()) else {
            continue;
        };
        // Skip cycle-broken back-edges (parent isn't actually left
        // of child in the layered output).
        if parent.x + parent.w >= child.x {
            continue;
        }
        edges_by_child
            .entry(e.from_table.as_str())
            .or_default()
            .push(e);
    }

    // Collect arrowhead positions so we can paint them AFTER the
    // card mask step — otherwise the destination card's left border
    // would overwrite the `▶` glyph and the user sees a wall of
    // truncated arrows.
    let mut arrowheads: Vec<(u16, u16)> = Vec::new();

    for (child_name, group) in &edges_by_child {
        let child = by_name[child_name];
        // Deterministic lane order: sort by parent name so the same
        // schema always renders the same way (helps users build
        // muscle memory and keeps tests stable).
        let mut sorted: Vec<&&RelationshipEdge> = group.iter().collect();
        sorted.sort_by(|a, b| a.to_table.cmp(&b.to_table));

        // Distribute lane bends in the gutter just left of `child.x`.
        // Lane 0 sits at child.x - 1; lane 1 at child.x - 2; …
        // Cap at h_gap.saturating_sub(1) so we never bend left of the
        // previous rank's right edge.
        let max_lane = h_gap.saturating_sub(1).max(1) as usize;
        // Distribute arrowhead rows across the child card's FULL
        // height so multiple parents fanning into the same table
        // show as distinct arrowheads stacked on the destination's
        // left edge rather than collapsing into one. The third paint
        // pass writes the `▶` glyph on top of any border cell it
        // lands on, so reusing border rows is safe.
        let card_top = child.y;
        let card_bottom = child.y + child.h.saturating_sub(1);
        let card_h = child.h.max(1);
        let n = sorted.len().max(1) as u16;
        for (i, edge) in sorted.iter().enumerate() {
            let parent = by_name[edge.to_table.as_str()];
            let src_x = parent.x + parent.w - 1;
            let src_y = parent.y + parent.h / 2;
            let dst_x = child.x;
            // Spread arrowhead rows evenly across the card. With
            // n incoming edges and a card of height card_h, lane i
            // lands at row card_top + (i * card_h / n).
            let row_offset = (i as u16).saturating_mul(card_h).saturating_div(n);
            let dst_y = (card_top + row_offset).min(card_bottom);
            let lane = i.min(max_lane.saturating_sub(1)) as u16;
            let bend_x = dst_x.saturating_sub(1 + lane);
            let bend_x = bend_x.max(src_x + 1);
            draw_lane_arrow(
                &mut grid, src_x, src_y, dst_x, dst_y, bend_x, th.accent2, th,
            );
            arrowheads.push((dst_x, dst_y));
            // FK column label rides just above the source-side leg.
            // Skip the label in Collapsed mode where pane budget is
            // already tight and the label would clutter the gutter.
            if zoom != Zoom::Collapsed && !edge.from_columns.is_empty() {
                let lbl = edge.from_columns.join(",");
                let lbl_y = src_y.saturating_sub(1);
                let lbl_x = src_x.saturating_add(2);
                put_text(
                    &mut grid,
                    lbl_x as usize,
                    lbl_y as usize,
                    &lbl,
                    th.muted,
                    false,
                );
            }
        }
    }

    // ── 5. Paint cards on top of edges so a card boundary always
    // wins visually over any arrow crossing it (mask step).
    for p in &layout_out.nodes {
        let info = inp.table_info.get(&p.name);
        let is_selected = inp.selected.map(|s| s == p.name).unwrap_or(false);
        let rows = card_rows(&p.name, info, zoom, th);
        let border = if is_selected { th.accent } else { th.border };
        draw_card(
            &mut grid,
            p.x as usize,
            p.y as usize,
            p.w as usize,
            p.h as usize,
            &rows,
            border,
            th.fg,
            is_selected,
            th,
        );
    }

    // ── 5b. Paint arrowheads last so they survive the card mask.
    // Each arrowhead lands ON the destination card's left border,
    // breaking the `│` with a `▶` exactly at the row where the
    // FK arrives. This is the conventional ERD look.
    for (x, y) in arrowheads {
        if let Some(row) = grid.get_mut(y as usize) {
            if let Some(cell) = row.get_mut(x as usize) {
                cell.ch = '▶';
                cell.fg = th.accent2;
                cell.bold = true;
            }
        }
    }

    // ── 6. Slice the visible window ──────────────────────────────
    let off_x = inp.viewport.offset_x.min(virt_w.saturating_sub(1)) as usize;
    let off_y = inp.viewport.offset_y.min(virt_h.saturating_sub(1)) as usize;
    let view_w = inp.view_w as usize;
    let view_h = inp.view_h as usize;
    let mut sub: Vec<Vec<Cell2>> = Vec::with_capacity(view_h);
    for r in 0..view_h {
        let src_y = off_y + r;
        let mut row: Vec<Cell2> = Vec::with_capacity(view_w);
        if src_y < grid.len() {
            for c in 0..view_w {
                let src_x = off_x + c;
                if src_x < grid[src_y].len() {
                    row.push(grid[src_y][src_x]);
                } else {
                    row.push(Cell2::space(th));
                }
            }
        } else {
            for _ in 0..view_w {
                row.push(Cell2::space(th));
            }
        }
        sub.push(row);
    }

    CanvasOutput {
        lines: grid_to_lines(&sub, th),
        virtual_w: virt_w,
        virtual_h: virt_h,
        placements: layout_out.nodes,
    }
}

/// Build the (label, colour, bold) rows for a card at the requested zoom.
fn card_rows(
    name: &str,
    info: Option<&TableInfo>,
    zoom: Zoom,
    th: Theme,
) -> Vec<(String, Color, bool)> {
    let mut rows: Vec<(String, Color, bool)> = Vec::new();
    rows.push((name.to_owned(), th.accent, true));
    match zoom {
        Zoom::Collapsed => {} // header only
        Zoom::Compact => {
            if let Some(i) = info {
                let pk_set: HashSet<&str> = i
                    .primary_key
                    .as_ref()
                    .map(|pk| pk.column_names.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                let fk_set: HashSet<&str> = i
                    .foreign_keys
                    .iter()
                    .flat_map(|fk| fk.column_names.iter().map(String::as_str))
                    .collect();
                let name_w = i
                    .columns
                    .iter()
                    .filter(|c| {
                        pk_set.contains(c.name.as_str()) || fk_set.contains(c.name.as_str())
                    })
                    .map(|c| c.name.len())
                    .max()
                    .unwrap_or(4);
                for c in i.columns.iter().filter(|c| {
                    pk_set.contains(c.name.as_str()) || fk_set.contains(c.name.as_str())
                }) {
                    let (marker, color) = if pk_set.contains(c.name.as_str()) {
                        ('★', th.warning)
                    } else {
                        ('⚷', th.accent2)
                    };
                    let label = format!("{marker} {:<w$}  {}", c.name, c.data_type, w = name_w);
                    rows.push((label, color, false));
                }
                if rows.len() == 1 {
                    rows.push(("(no keys)".to_owned(), th.muted, false));
                }
            } else {
                rows.push(("(loading…)".to_owned(), th.muted, false));
            }
        }
        Zoom::Full => {
            if let Some(i) = info {
                let pk_set: HashSet<&str> = i
                    .primary_key
                    .as_ref()
                    .map(|pk| pk.column_names.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                let fk_set: HashSet<&str> = i
                    .foreign_keys
                    .iter()
                    .flat_map(|fk| fk.column_names.iter().map(String::as_str))
                    .collect();
                let name_w = i.columns.iter().map(|c| c.name.len()).max().unwrap_or(4);
                for c in &i.columns {
                    let (marker, color) = if pk_set.contains(c.name.as_str()) {
                        ('★', th.warning)
                    } else if fk_set.contains(c.name.as_str()) {
                        ('⚷', th.accent2)
                    } else {
                        (' ', th.fg)
                    };
                    let label = format!("{marker} {:<w$}  {}", c.name, c.data_type, w = name_w);
                    rows.push((label, color, false));
                }
            } else {
                rows.push(("(loading…)".to_owned(), th.muted, false));
            }
        }
    }
    rows
}

/// Like [`draw_arrow`] but uses an explicit `mid_x` for the vertical
/// leg so multiple parallel edges can bend in different gutters.
#[allow(clippy::too_many_arguments)]
fn draw_lane_arrow(
    grid: &mut [Vec<Cell2>],
    x1: u16,
    y1: u16,
    x2: u16,
    y2: u16,
    mid_x: u16,
    color: Color,
    th: Theme,
) {
    if x2 <= x1 {
        return;
    }
    // Fall back to the existing draw_arrow when the requested mid_x
    // equals the natural midpoint — it handles labels and arrowheads.
    let natural = x1 + (x2 - x1) / 2;
    if mid_x == natural {
        draw_arrow(
            grid,
            x1 as usize,
            y1 as usize,
            x2 as usize,
            y2 as usize,
            "",
            color,
            false,
            th,
        );
        return;
    }
    // Otherwise paint a three-leg orthogonal route ourselves.
    // Leg 1: horizontal x1 → mid_x at y1.
    paint_h(
        grid,
        x1.min(mid_x) as usize,
        x1.max(mid_x) as usize,
        y1 as usize,
        color,
    );
    // Leg 2: vertical y1 → y2 at mid_x.
    paint_v(
        grid,
        mid_x as usize,
        y1.min(y2) as usize,
        y1.max(y2) as usize,
        color,
    );
    // Leg 3: horizontal mid_x → x2 at y2.
    paint_h(
        grid,
        mid_x.min(x2) as usize,
        mid_x.max(x2) as usize,
        y2 as usize,
        color,
    );
    // Corners.
    let c1 = if y1 < y2 { '╮' } else { '╯' };
    let c2 = if y1 < y2 { '╰' } else { '╭' };
    put_cell(grid, mid_x as usize, y1 as usize, c1, color);
    put_cell(grid, mid_x as usize, y2 as usize, c2, color);
    // Arrowhead.
    put_cell(grid, x2 as usize, y2 as usize, '▶', color);
}

fn paint_h(grid: &mut [Vec<Cell2>], x1: usize, x2: usize, y: usize, color: Color) {
    for x in x1..=x2 {
        put_cell(grid, x, y, '─', color);
    }
}

fn paint_v(grid: &mut [Vec<Cell2>], x: usize, y1: usize, y2: usize, color: Color) {
    for y in y1..=y2 {
        put_cell(grid, x, y, '│', color);
    }
}

fn put_cell(grid: &mut [Vec<Cell2>], x: usize, y: usize, ch: char, fg: Color) {
    if let Some(row) = grid.get_mut(y) {
        if let Some(cell) = row.get_mut(x) {
            *cell = Cell2 {
                ch,
                fg,
                bold: false,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsqlx_db::{ColumnInfo, ForeignKeyInfo, PrimaryKeyInfo, RelationshipEdge, TableInfo};

    fn theme() -> Theme {
        Theme::catppuccin_mocha()
    }

    fn lines_to_string(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_schema_yields_blank_canvas() {
        let info: HashMap<String, TableInfo> = HashMap::new();
        let vp = Viewport::new();
        let out = render_schema_canvas(CanvasInput {
            tables: &[],
            table_info: &info,
            edges: &[],
            selected: None,
            viewport: &vp,
            view_w: 40,
            view_h: 10,
            theme: theme(),
        });
        assert_eq!(out.lines.len(), 10);
        assert!(out.virtual_w >= 1);
        assert!(out.virtual_h >= 1);
    }

    fn ti(name: &str, cols: &[(&str, &str)], pk: &[&str], fks: &[(&str, &str)]) -> TableInfo {
        TableInfo {
            name: name.to_owned(),
            schema: "public".to_owned(),
            columns: cols
                .iter()
                .map(|(n, t)| ColumnInfo {
                    name: (*n).to_owned(),
                    data_type: (*t).to_owned(),
                    is_nullable: false,
                    default_value: None,
                })
                .collect(),
            indexes: vec![],
            primary_key: if pk.is_empty() {
                None
            } else {
                Some(PrimaryKeyInfo {
                    name: "pk".to_owned(),
                    column_names: pk.iter().map(|s| (*s).to_owned()).collect(),
                })
            },
            foreign_keys: fks
                .iter()
                .map(|(col, ref_t)| ForeignKeyInfo {
                    name: format!("fk_{col}"),
                    column_names: vec![(*col).to_owned()],
                    referenced_table: (*ref_t).to_owned(),
                    referenced_columns: vec!["id".to_owned()],
                })
                .collect(),
            constraints: vec![],
        }
    }

    #[test]
    fn full_zoom_renders_every_column_of_focused_table() {
        // 12 columns — well past the focused view's 8-column cap.
        let cols: Vec<(&str, &str)> = (0..12)
            .map(|i| {
                (
                    Box::leak(format!("c{i:02}").into_boxed_str()) as &str,
                    "int",
                )
            })
            .collect();
        let t = ti("big", &cols, &["c00"], &[]);
        let mut info = HashMap::new();
        info.insert("big".to_owned(), t);
        let mut vp = Viewport::new();
        vp.zoom = Zoom::Full;
        let out = render_schema_canvas(CanvasInput {
            tables: &["big".to_owned()],
            table_info: &info,
            edges: &[],
            selected: Some("big"),
            viewport: &vp,
            view_w: 80,
            view_h: 30,
            theme: theme(),
        });
        let dump = lines_to_string(&out.lines);
        for i in 0..12 {
            let needle = format!("c{i:02}");
            assert!(
                dump.contains(&needle),
                "Full zoom must render column {needle}; canvas was:\n{dump}"
            );
        }
    }

    #[test]
    fn three_tables_chain_renders_two_arrows() {
        let tables = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let mut info = HashMap::new();
        info.insert("a".to_owned(), ti("a", &[("id", "int")], &["id"], &[]));
        info.insert(
            "b".to_owned(),
            ti(
                "b",
                &[("id", "int"), ("a_id", "int")],
                &["id"],
                &[("a_id", "a")],
            ),
        );
        info.insert(
            "c".to_owned(),
            ti(
                "c",
                &[("id", "int"), ("b_id", "int")],
                &["id"],
                &[("b_id", "b")],
            ),
        );
        let edges = vec![
            RelationshipEdge {
                from_table: "b".to_owned(),
                from_columns: vec!["a_id".to_owned()],
                to_table: "a".to_owned(),
                to_columns: vec!["id".to_owned()],
            },
            RelationshipEdge {
                from_table: "c".to_owned(),
                from_columns: vec!["b_id".to_owned()],
                to_table: "b".to_owned(),
                to_columns: vec!["id".to_owned()],
            },
        ];
        let vp = Viewport::new();
        // Big enough virtual canvas to fit the whole chain visibly.
        let out = render_schema_canvas(CanvasInput {
            tables: &tables,
            table_info: &info,
            edges: &edges,
            selected: None,
            viewport: &vp,
            view_w: 200,
            view_h: 30,
            theme: theme(),
        });
        let dump = lines_to_string(&out.lines);
        assert!(dump.contains("a"), "table a rendered");
        assert!(dump.contains("b"), "table b rendered");
        assert!(dump.contains("c"), "table c rendered");
        assert!(
            dump.matches('▶').count() >= 2,
            "two arrowheads — got\n{dump}"
        );
    }

    /// Three parents (`a`, `b`, `c`) all reference the same child
    /// (`d`). Each incoming edge must produce its own arrowhead;
    /// the lane allocator should stagger their bend points so the
    /// vertical legs don't overlap into a single visible line.
    #[test]
    fn fan_in_renders_three_distinct_arrowheads() {
        let tables: Vec<String> = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let mut info: HashMap<String, TableInfo> = HashMap::new();
        for t in &tables {
            info.insert(t.clone(), ti(t, &[("id", "int")], &["id"], &[]));
        }
        // Three parent→child edges all converging on `d`.
        let edges = vec![
            RelationshipEdge {
                from_table: "d".into(),
                from_columns: vec!["a_id".into()],
                to_table: "a".into(),
                to_columns: vec!["id".into()],
            },
            RelationshipEdge {
                from_table: "d".into(),
                from_columns: vec!["b_id".into()],
                to_table: "b".into(),
                to_columns: vec!["id".into()],
            },
            RelationshipEdge {
                from_table: "d".into(),
                from_columns: vec!["c_id".into()],
                to_table: "c".into(),
                to_columns: vec!["id".into()],
            },
        ];
        let vp = Viewport::new();
        let out = render_schema_canvas(CanvasInput {
            tables: &tables,
            table_info: &info,
            edges: &edges,
            selected: None,
            viewport: &vp,
            view_w: 200,
            view_h: 30,
            theme: theme(),
        });
        let dump = lines_to_string(&out.lines);
        assert_eq!(
            dump.matches('▶').count(),
            3,
            "three arrowheads expected (one per FK) — got\n{dump}"
        );
    }

    /// Collapsed zoom must shrink the virtual canvas vs Compact for
    /// the same schema — otherwise the "see everything" zoom level
    /// isn't actually doing anything useful.
    #[test]
    fn collapsed_zoom_shrinks_virtual_canvas_vs_compact() {
        let tables: Vec<String> = (0..5).map(|i| format!("t{i}")).collect();
        let mut info: HashMap<String, TableInfo> = HashMap::new();
        for t in &tables {
            info.insert(
                t.clone(),
                ti(
                    t,
                    &[
                        ("id", "int"),
                        ("name", "text"),
                        ("created_at", "timestamptz"),
                        ("updated_at", "timestamptz"),
                    ],
                    &["id"],
                    &[],
                ),
            );
        }
        // Linear FK chain so all 5 tables land in 5 different ranks.
        let edges: Vec<RelationshipEdge> = (1..5)
            .map(|i| RelationshipEdge {
                from_table: format!("t{i}"),
                from_columns: vec![format!("t{}_id", i - 1)],
                to_table: format!("t{}", i - 1),
                to_columns: vec!["id".into()],
            })
            .collect();
        let mut vp_compact = Viewport::new(); // default Compact
        let out_compact = render_schema_canvas(CanvasInput {
            tables: &tables,
            table_info: &info,
            edges: &edges,
            selected: None,
            viewport: &vp_compact,
            view_w: 60,
            view_h: 20,
            theme: theme(),
        });
        vp_compact.zoom = Zoom::Collapsed;
        let out_collapsed = render_schema_canvas(CanvasInput {
            tables: &tables,
            table_info: &info,
            edges: &edges,
            selected: None,
            viewport: &vp_compact,
            view_w: 60,
            view_h: 20,
            theme: theme(),
        });
        assert!(
            out_collapsed.virtual_w < out_compact.virtual_w,
            "Collapsed should pack tighter horizontally: \
             collapsed={} compact={}",
            out_collapsed.virtual_w,
            out_compact.virtual_w,
        );
        assert!(
            out_collapsed.virtual_h <= out_compact.virtual_h,
            "Collapsed should not be taller: collapsed={} compact={}",
            out_collapsed.virtual_h,
            out_compact.virtual_h,
        );
    }
}
