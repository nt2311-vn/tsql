//! Pure rendering primitives for ERD canvases: a cell grid, a card
//! drawer, an orthogonal-arrow drawer, and a converter to ratatui
//! `Line`s. No `AppState`, no I/O — easy to unit-test in isolation
//! and reusable by the focused view and (PR #2) the whole-schema
//! canvas view.

use std::collections::HashSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tsqlx_db::{RelationshipEdge, TableInfo};

use crate::Theme;

/// Single canvas cell. `bold` is the only modifier we need so far —
/// we keep the type tiny so the grid stays cache-friendly.
#[derive(Clone, Copy)]
pub(crate) struct Cell2 {
    pub ch: char,
    pub fg: Color,
    pub bold: bool,
}

impl Cell2 {
    pub(crate) fn space(th: Theme) -> Self {
        Self {
            ch: ' ',
            fg: th.fg,
            bold: false,
        }
    }
}

/// Build the focused-graph canvas. Returns one styled `Line` per row.
///
/// `centre_scroll` is the index of the first column to render inside
/// the centre card. Pass `0` for "from the top". When the available
/// height can't hold every remaining column, the function appends
/// `↑ N hidden above` / `↓ M hidden below` indicators so the user
/// knows scrolling is meaningful.
#[allow(clippy::too_many_arguments)]
pub fn render_focus_canvas(
    width: u16,
    height: u16,
    centre_name: &str,
    centre_info: Option<&TableInfo>,
    incoming: &[&RelationshipEdge],
    outgoing: &[&RelationshipEdge],
    centre_scroll: usize,
    th: Theme,
) -> Vec<Line<'static>> {
    let w = width as usize;
    let h = height as usize;
    let mut grid: Vec<Vec<Cell2>> = vec![vec![Cell2::space(th); w]; h];

    // ── centre card ────────────────────────────────────────────────
    // Build rows: header (table name) + horizontal rule + columns.
    // Columns past the available card height get truncated with a
    // "↑ N hidden above" / "↓ N hidden below" marker so the user
    // knows the centre card has more to scroll through (J/K).
    let mut centre_rows: Vec<(String, Color, bool)> = Vec::new();
    centre_rows.push((centre_name.to_owned(), th.accent, true));
    if let Some(info) = centre_info {
        let pk_set: HashSet<&str> = info
            .primary_key
            .as_ref()
            .map(|pk| pk.column_names.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let fk_set: HashSet<&str> = info
            .foreign_keys
            .iter()
            .flat_map(|fk| fk.column_names.iter().map(String::as_str))
            .collect();
        let name_w = info.columns.iter().map(|c| c.name.len()).max().unwrap_or(4);
        // Cap visible columns by the pane height: header + rule + N
        // columns + 2 borders. Leave one slot for each indicator we
        // might need.
        let total = info.columns.len();
        let scroll = centre_scroll.min(total.saturating_sub(1));
        // Visible budget = h - header - borders. We compute it
        // generously here; `draw_card` will clip if there's overflow.
        // Reserve up to 2 lines for the hidden-above / hidden-below
        // indicators.
        let body_budget = h.saturating_sub(4); // borders + header + rule
        let above = scroll;
        let need_above_marker = above > 0;
        let mut visible_rows = body_budget;
        if need_above_marker {
            visible_rows = visible_rows.saturating_sub(1);
        }
        // Tentatively assume we won't need a below-marker. Decide
        // after slicing.
        let end = (scroll + visible_rows).min(total);
        let below = total.saturating_sub(end);
        let need_below_marker = below > 0;
        let end = if need_below_marker {
            // Reserve one row for the marker by shrinking the window.
            (scroll + visible_rows.saturating_sub(1)).min(total)
        } else {
            end
        };
        let below = total.saturating_sub(end);
        if need_above_marker {
            centre_rows.push((format!("↑ {above} hidden above"), th.muted, false));
        }
        for c in &info.columns[scroll..end] {
            let (marker, color) = if pk_set.contains(c.name.as_str()) {
                ('★', th.warning)
            } else if fk_set.contains(c.name.as_str()) {
                ('⚷', th.accent2)
            } else {
                (' ', th.fg)
            };
            let label = format!("{marker} {:<w$}  {}", c.name, c.data_type, w = name_w);
            centre_rows.push((label, color, false));
        }
        if below > 0 {
            centre_rows.push((format!("↓ {below} hidden below"), th.muted, false));
        }
    } else {
        centre_rows.push(("(loading…)".to_owned(), th.muted, false));
    }

    // Side card width scales with available width. We never go below
    // 14 (room for ~10 chars of table name) since otherwise the box
    // truncates short table names like `customers` mid-word.
    let side_box_w: usize = if w < 80 {
        14
    } else if w < 110 {
        16
    } else {
        18
    };
    let side_box_h: usize = 3;
    let arrow_pad: usize = 4;

    // Box width = max content + 2 padding. Cap so neighbours fit if at
    // all possible — at narrow widths we'd rather truncate the centre
    // card body than push neighbour cards off-screen.
    let max_centre_w = centre_rows
        .iter()
        .map(|(s, _, _)| s.chars().count())
        .max()
        .unwrap_or(8);
    // Reserve enough room for at least one side + arrow when w >= 50.
    let reserved = if w >= 50 {
        side_box_w + arrow_pad + 2
    } else {
        4
    };
    let centre_box_w = (max_centre_w + 4)
        .min(w.saturating_sub(reserved))
        .max(centre_name.chars().count() + 4);
    let centre_box_h = (centre_rows.len() + 2).min(h);

    // Decide how many neighbours we can show vertically with 1-row gap.
    let stack_capacity = (h.saturating_sub(1) / (side_box_h + 1)).max(1);
    let lefts: Vec<&&RelationshipEdge> = incoming.iter().take(stack_capacity).collect();
    let rights: Vec<&&RelationshipEdge> = outgoing.iter().take(stack_capacity).collect();

    // Lay out columns horizontally. If the pane is too narrow drop
    // neighbours first; centre always renders.
    let needed_w_both = side_box_w + arrow_pad + centre_box_w + arrow_pad + side_box_w;
    let needed_w_one = side_box_w + arrow_pad + centre_box_w;
    let (show_left, show_right) = if w >= needed_w_both {
        (!lefts.is_empty(), !rights.is_empty())
    } else if w >= needed_w_one {
        // Only one side fits — prefer outgoing (what the table depends on).
        if !rights.is_empty() {
            (false, true)
        } else {
            (!lefts.is_empty(), false)
        }
    } else {
        (false, false)
    };

    let centre_x = (w.saturating_sub(centre_box_w)) / 2;
    let centre_y = (h.saturating_sub(centre_box_h)) / 2;
    let left_x = 0usize;
    let right_x = w.saturating_sub(side_box_w);

    // Draw centre card.
    draw_card(
        &mut grid,
        centre_x,
        centre_y,
        centre_box_w,
        centre_box_h,
        &centre_rows,
        th.accent,
        th.accent,
        true,
        th,
    );

    // Helper: distribute n boxes evenly across the available height.
    let distribute = |n: usize| -> Vec<usize> {
        if n == 0 {
            return Vec::new();
        }
        let total_h = n * side_box_h + n.saturating_sub(1);
        if total_h >= h {
            (0..n).map(|i| i * (side_box_h + 1)).collect()
        } else {
            let start = (h - total_h) / 2;
            (0..n).map(|i| start + i * (side_box_h + 1)).collect()
        }
    };

    // Left side: incoming FKs ("X.fk_col → centre.pk").
    if show_left {
        let ys = distribute(lefts.len());
        for (i, edge) in lefts.iter().enumerate() {
            let y = ys[i];
            let rows = vec![
                (edge.from_table.clone(), th.accent, true),
                (
                    edge.from_columns
                        .join(",")
                        .chars()
                        .take(side_box_w - 4)
                        .collect::<String>(),
                    th.accent2,
                    false,
                ),
            ];
            draw_card(
                &mut grid, left_x, y, side_box_w, side_box_h, &rows, th.border, th.fg, false, th,
            );
            // Arrow from right edge of left box → left edge of centre,
            // anchored to the vertical mid of the source box and the
            // matching column row of the centre (or its header if we
            // can't find one).
            let src_y = y + side_box_h / 2;
            let dst_y = centre_y + 1 + locate_centre_row(&centre_rows, &edge.to_columns);
            let label = edge.from_columns.join(",");
            draw_arrow(
                &mut grid,
                left_x + side_box_w - 1,
                src_y,
                centre_x,
                dst_y,
                &label,
                th.accent2,
                false, // ► points right
                th,
            );
        }
    }

    // Right side: outgoing FKs ("centre.fk_col → Y.pk").
    if show_right {
        let ys = distribute(rights.len());
        for (i, edge) in rights.iter().enumerate() {
            let y = ys[i];
            let rows = vec![
                (edge.to_table.clone(), th.accent, true),
                (
                    edge.to_columns
                        .join(",")
                        .chars()
                        .take(side_box_w - 4)
                        .collect::<String>(),
                    th.warning,
                    false,
                ),
            ];
            draw_card(
                &mut grid, right_x, y, side_box_w, side_box_h, &rows, th.border, th.fg, false, th,
            );
            let src_y = centre_y + 1 + locate_centre_row(&centre_rows, &edge.from_columns);
            let dst_y = y + side_box_h / 2;
            let label = edge.from_columns.join(",");
            draw_arrow(
                &mut grid,
                centre_x + centre_box_w - 1,
                src_y,
                right_x,
                dst_y,
                &label,
                th.accent,
                false,
                th,
            );
        }
    }

    // ── footer hint ───────────────────────────────────────────────
    let hint_y = h.saturating_sub(1);
    let stats = format!(
        "  ←{} incoming   {} outgoing→   {} neighbours hidden",
        incoming.len(),
        outgoing.len(),
        incoming.len().saturating_sub(lefts.len()) + outgoing.len().saturating_sub(rights.len()),
    );
    put_text(&mut grid, 0, hint_y, &stats, th.muted, false);

    // ── convert grid → ratatui Lines ──────────────────────────────
    grid_to_lines(&grid, th)
}

/// Find the row offset in the centre card that matches the first
/// referenced column name, so the connecting arrow lands on it. Falls
/// back to the header row when no match is found.
pub(crate) fn locate_centre_row(rows: &[(String, Color, bool)], cols: &[String]) -> usize {
    if cols.is_empty() {
        return 0;
    }
    let target = &cols[0];
    for (i, (label, _, _)) in rows.iter().enumerate().skip(1) {
        // Centre rows look like "★ name  type". Match on whitespace-
        // delimited second token.
        let mut parts = label.split_whitespace();
        let _marker = parts.next();
        if let Some(name) = parts.next() {
            if name == target {
                return i;
            }
        }
    }
    0
}

/// Draw a rounded card with a header row + body lines. `header_color`
/// styles the title and (when `emphasise` is set) the border.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_card(
    grid: &mut [Vec<Cell2>],
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    rows: &[(String, Color, bool)],
    border: Color,
    _body_color: Color,
    emphasise: bool,
    th: Theme,
) {
    if w < 4 || h < 3 {
        return;
    }
    let h_max = grid.len();
    let w_max = grid.first().map(Vec::len).unwrap_or(0);
    if y >= h_max || x >= w_max {
        return;
    }
    let h = h.min(h_max - y);
    let w = w.min(w_max - x);

    let (tl, tr, bl, br, hch, vch) = if emphasise {
        ('╭', '╮', '╰', '╯', '─', '│')
    } else {
        ('┌', '┐', '└', '┘', '─', '│')
    };
    // Top + bottom borders.
    put(grid, x, y, tl, border, false);
    put(grid, x + w - 1, y, tr, border, false);
    put(grid, x, y + h - 1, bl, border, false);
    put(grid, x + w - 1, y + h - 1, br, border, false);
    for i in 1..w - 1 {
        put(grid, x + i, y, hch, border, false);
        put(grid, x + i, y + h - 1, hch, border, false);
    }
    for j in 1..h - 1 {
        put(grid, x, y + j, vch, border, false);
        put(grid, x + w - 1, y + j, vch, border, false);
    }

    // Inner rows.
    for (i, (text, color, bold)) in rows.iter().enumerate() {
        let row_y = y + 1 + i;
        if row_y >= y + h - 1 {
            break;
        }
        let inner_w = w - 2;
        let truncated: String = text.chars().take(inner_w.saturating_sub(2)).collect();
        put_text(grid, x + 2, row_y, &truncated, *color, *bold);
        // Insert a horizontal rule under the header.
        if i == 0 && rows.len() > 1 && row_y + 1 < y + h - 1 {
            for k in 1..w - 1 {
                put(grid, x + k, row_y + 1, '─', th.muted, false);
            }
        }
    }
}

/// Draw a one-cell-thick orthogonal arrow from (x1,y1) to (x2,y2) with
/// a centred label. Uses box-drawing chars so it composites cleanly
/// over existing cells. `back_arrow` swaps the arrowhead direction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_arrow(
    grid: &mut [Vec<Cell2>],
    x1: usize,
    y1: usize,
    x2: usize,
    y2: usize,
    label: &str,
    color: Color,
    back_arrow: bool,
    _th: Theme,
) {
    if x2 <= x1 || grid.is_empty() {
        return;
    }
    let mid_x = x1 + (x2 - x1) / 2;
    // First leg: horizontal from x1 to mid_x at y1.
    for x in x1..=mid_x {
        put(grid, x, y1, '─', color, false);
    }
    // Vertical: y1 -> y2 at mid_x.
    if y1 != y2 {
        let (lo, hi) = if y1 < y2 { (y1, y2) } else { (y2, y1) };
        for y in lo..=hi {
            // Don't overwrite the corners we'll set below.
            if y != y1 && y != y2 {
                put(grid, mid_x, y, '│', color, false);
            }
        }
        // Corners.
        let c1 = if y1 < y2 { '╮' } else { '╯' };
        let c2 = if y1 < y2 { '╰' } else { '╭' };
        put(grid, mid_x, y1, c1, color, false);
        put(grid, mid_x, y2, c2, color, false);
    }
    // Second leg: horizontal from mid_x to x2 at y2.
    for x in mid_x..=x2 {
        put(grid, x, y2, '─', color, false);
    }
    // Arrowhead.
    let head_x = if back_arrow { x1 } else { x2 };
    let head_y = if back_arrow { y1 } else { y2 };
    let head_ch = if back_arrow { '◀' } else { '▶' };
    put(grid, head_x, head_y, head_ch, color, true);

    // Label sits one row above the horizontal leg with the longest run.
    if !label.is_empty() {
        let label_chars: Vec<char> = label.chars().collect();
        let lbl_y = if y1 == y2 {
            y1.saturating_sub(1)
        } else {
            // Centre on the longer leg; pick the source leg.
            y1.saturating_sub(1)
        };
        let lbl_x = x1 + 1;
        for (i, ch) in label_chars.iter().enumerate() {
            if lbl_x + i >= x2 {
                break;
            }
            put(grid, lbl_x + i, lbl_y, *ch, color, false);
        }
    }
}

#[inline]
pub(crate) fn put(grid: &mut [Vec<Cell2>], x: usize, y: usize, ch: char, fg: Color, bold: bool) {
    if let Some(row) = grid.get_mut(y) {
        if let Some(cell) = row.get_mut(x) {
            *cell = Cell2 { ch, fg, bold };
        }
    }
}

pub(crate) fn put_text(
    grid: &mut [Vec<Cell2>],
    x: usize,
    y: usize,
    text: &str,
    fg: Color,
    bold: bool,
) {
    for (i, ch) in text.chars().enumerate() {
        put(grid, x + i, y, ch, fg, bold);
    }
}

pub(crate) fn grid_to_lines(grid: &[Vec<Cell2>], th: Theme) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::with_capacity(grid.len());
    for row in grid {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut buf = String::new();
        let mut cur_fg = th.fg;
        let mut cur_bold = false;
        for cell in row {
            if cell.fg != cur_fg || cell.bold != cur_bold {
                if !buf.is_empty() {
                    let mut style = Style::default().fg(cur_fg).bg(th.bg);
                    if cur_bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                cur_fg = cell.fg;
                cur_bold = cell.bold;
            }
            buf.push(cell.ch);
        }
        if !buf.is_empty() {
            let mut style = Style::default().fg(cur_fg).bg(th.bg);
            if cur_bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(buf, style));
        }
        out.push(Line::from(spans));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tsqlx_db::{ColumnInfo, PrimaryKeyInfo};

    fn theme() -> Theme {
        Theme::catppuccin_mocha()
    }

    fn dump(lines: &[Line<'static>]) -> String {
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

    fn big_table(n: usize) -> TableInfo {
        TableInfo {
            name: "big".to_owned(),
            schema: "public".to_owned(),
            columns: (0..n)
                .map(|i| ColumnInfo {
                    name: format!("c{i:02}"),
                    data_type: "int".to_owned(),
                    is_nullable: false,
                    default_value: None,
                })
                .collect(),
            indexes: vec![],
            primary_key: Some(PrimaryKeyInfo {
                name: "pk".to_owned(),
                column_names: vec!["c00".to_owned()],
            }),
            foreign_keys: vec![],
            constraints: vec![],
        }
    }

    /// A tall pane (height = 30) should now show *every* column of a
    /// 20-column table — there is no 8-column cap any more. The old
    /// behaviour appended `… (+12 more)`; the new behaviour fits all
    /// 20 in the card body.
    #[test]
    fn tall_pane_renders_all_twenty_columns_no_eight_cap() {
        let t = big_table(20);
        let lines = render_focus_canvas(80, 30, "big", Some(&t), &[], &[], 0, theme());
        let d = dump(&lines);
        for i in 0..20 {
            let needle = format!("c{i:02}");
            assert!(
                d.contains(&needle),
                "column {needle} must render; canvas:\n{d}"
            );
        }
        assert!(
            !d.contains("(+12 more)"),
            "old truncation marker must not appear; canvas:\n{d}"
        );
    }

    /// When the pane is short, the card body fits ~3-4 columns and
    /// the renderer announces what is hidden so the user knows
    /// J/K means something.
    #[test]
    fn short_pane_shows_hidden_below_marker() {
        let t = big_table(20);
        // h=10 → body_budget = 6, no above marker, ~5 rows + below marker.
        let lines = render_focus_canvas(80, 10, "big", Some(&t), &[], &[], 0, theme());
        let d = dump(&lines);
        assert!(d.contains("c00"), "first column visible");
        assert!(d.contains("hidden below"), "below-marker present: {d}");
    }

    /// Scrolling into the middle of a tall table must surface BOTH
    /// "hidden above" and "hidden below" markers and skip the first
    /// `scroll` columns from the visible window.
    #[test]
    fn middle_scroll_shows_both_above_and_below_markers() {
        let t = big_table(20);
        let lines = render_focus_canvas(80, 10, "big", Some(&t), &[], &[], 5, theme());
        let d = dump(&lines);
        assert!(d.contains("hidden above"), "above marker present: {d}");
        assert!(d.contains("hidden below"), "below marker present: {d}");
        assert!(!d.contains("c00"), "scrolled past c00: {d}");
        assert!(d.contains("c05") || d.contains("c06"), "shows middle: {d}");
    }
}
