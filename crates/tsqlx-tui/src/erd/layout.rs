//! Layered (Sugiyama-style) layout for the whole-schema ERD canvas.
//!
//! Tables become columns of cards flowing left → right by FK depth.
//! Roots (no incoming FKs) sit on the left; the deepest dependants on
//! the right. Within each rank cards are ordered by the barycentre of
//! their neighbours' positions so edges cross as little as possible.
//!
//! The output is pure pixel-grid coordinates in "character cells";
//! the caller (`canvas`) paints into them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use tsqlx_db::RelationshipEdge;

/// Pixel-grid placement of one table card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePlacement {
    pub name: String,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// Result of [`layered_layout`]: every table has a slot in the virtual
/// canvas (`virtual_w` × `virtual_h`).
#[derive(Debug, Clone)]
pub struct LayoutOut {
    pub nodes: Vec<NodePlacement>,
    pub virtual_w: u16,
    pub virtual_h: u16,
}

impl LayoutOut {
    /// Look up a placement by table name. O(n) — fine for typical
    /// schema sizes. Used by tests and (soon) the `c` recentre key.
    #[allow(dead_code)]
    pub fn get(&self, name: &str) -> Option<&NodePlacement> {
        self.nodes.iter().find(|p| p.name == name)
    }
}

/// Compute a layered layout.
///
/// * `tables`  — all table names to lay out (order doesn't matter; we
///   sort within a rank by barycentre).
/// * `edges`   — FK edges; `from_table`/`to_table` reference tables in
///   `tables`. Edges to unknown tables are ignored.
/// * `size_of` — `(width, height)` in character cells for each table
///   card. Lets the caller drive layout by current zoom level.
/// * `h_gap`   — horizontal gap between adjacent ranks.
/// * `v_gap`   — vertical gap between cards within the same rank.
pub fn layered_layout<F>(
    tables: &[String],
    edges: &[RelationshipEdge],
    size_of: F,
    h_gap: u16,
    v_gap: u16,
) -> LayoutOut
where
    F: Fn(&str) -> (u16, u16),
{
    if tables.is_empty() {
        return LayoutOut {
            nodes: Vec::new(),
            virtual_w: 0,
            virtual_h: 0,
        };
    }

    // ── 1. Build the FK graph restricted to known tables ──────────
    // Edge orientation: the *referenced* (parent) table flows into the
    // *referencing* (dependant) table, so parents end up on the left
    // and dependants on the right. `from_table` is the dependant in a
    // `RelationshipEdge`, `to_table` is the parent.
    let known: HashSet<&str> = tables.iter().map(String::as_str).collect();
    // `out[parent]` = tables that depend on `parent` (have an FK to it).
    // `in_count[dependant]` = how many parents this table references.
    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_count: HashMap<&str, usize> = HashMap::new();
    for t in tables {
        out.insert(t.as_str(), Vec::new());
        in_count.insert(t.as_str(), 0);
    }
    let mut seen_edge: HashSet<(&str, &str)> = HashSet::new();
    for e in edges {
        let dependant = e.from_table.as_str();
        let parent = e.to_table.as_str();
        if dependant == parent {
            continue; // self-FKs don't influence ranking
        }
        if !known.contains(dependant) || !known.contains(parent) {
            continue;
        }
        if !seen_edge.insert((parent, dependant)) {
            continue;
        }
        out.get_mut(parent).unwrap().push(dependant);
        *in_count.get_mut(dependant).unwrap() += 1;
    }

    // ── 2. Rank assignment via Kahn-style longest-path layering ───
    // Cycle handling: when the queue empties but some nodes remain
    // unassigned, pick the unassigned node with the lowest remaining
    // in-degree (lex tie-break for determinism) and start it at
    // rank 0 — that breaks the back-edge cleanly.
    let mut remaining_in = in_count.clone();
    let mut rank: HashMap<&str, usize> = HashMap::new();
    let mut drained: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = tables
        .iter()
        .map(String::as_str)
        .filter(|t| remaining_in[t] == 0)
        .collect();
    for &t in &queue {
        rank.insert(t, 0);
    }
    while drained.len() < tables.len() {
        while let Some(n) = queue.pop_front() {
            if !drained.insert(n) {
                continue; // already processed
            }
            let r = rank[n];
            for &m in &out[n] {
                let cur = rank.get(m).copied().unwrap_or(0);
                rank.insert(m, (r + 1).max(cur));
                let c = remaining_in.get_mut(m).unwrap();
                *c = c.saturating_sub(1);
                if *c == 0 && !drained.contains(m) {
                    queue.push_back(m);
                }
            }
        }
        if drained.len() >= tables.len() {
            break;
        }
        // Cycle: pick the unassigned (un-drained) node with the
        // lowest remaining in-degree, anchor it at a fresh rank, and
        // resume. Lex tie-break for determinism.
        let pick = tables
            .iter()
            .map(String::as_str)
            .filter(|t| !drained.contains(*t))
            .min_by_key(|&t| (remaining_in[t], t));
        match pick {
            Some(p) => {
                rank.entry(p).or_insert(0);
                remaining_in.insert(p, 0);
                queue.push_back(p);
            }
            None => break,
        }
    }

    // ── 3. Group by rank, sorted by name as the tie-breaker seed ──
    let mut by_rank: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for t in tables {
        by_rank
            .entry(rank[t.as_str()])
            .or_default()
            .push(t.as_str());
    }
    for v in by_rank.values_mut() {
        v.sort();
    }

    // ── 4. Barycentre ordering: two sweeps to cut crossings ──────
    // Build undirected adjacency for barycentre (both in & out
    // neighbours count, since edges curve both ways visually).
    let mut neigh: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in tables {
        neigh.insert(t.as_str(), Vec::new());
    }
    for e in edges {
        let a = e.from_table.as_str();
        let b = e.to_table.as_str();
        if a == b || !known.contains(a) || !known.contains(b) {
            continue;
        }
        neigh.get_mut(a).unwrap().push(b);
        neigh.get_mut(b).unwrap().push(a);
    }

    let ranks_sorted: Vec<usize> = by_rank.keys().copied().collect();
    // Two sweeps (down then up). Empirically enough for typical
    // schemas to converge.
    for _ in 0..2 {
        for &r in &ranks_sorted {
            // Index map from current ordering.
            let positions: HashMap<&str, usize> = by_rank
                .values()
                .flat_map(|v| v.iter().enumerate().map(|(i, &n)| (n, i)))
                .collect();
            let nodes = by_rank.get_mut(&r).unwrap();
            let mut keyed: Vec<(f64, &str)> = nodes
                .iter()
                .map(|&n| {
                    let bary = barycentre(n, &neigh, &positions);
                    (bary, n)
                })
                .collect();
            keyed.sort_by(|a, b| {
                a.0.partial_cmp(&b.0)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.1.cmp(b.1))
            });
            *nodes = keyed.into_iter().map(|(_, n)| n).collect();
        }
    }

    // ── 5. Pixel layout ──────────────────────────────────────────
    let mut placements: Vec<NodePlacement> = Vec::with_capacity(tables.len());
    let mut x_cursor: u16 = 0;
    let mut max_y: u16 = 0;
    for &r in &ranks_sorted {
        let col = &by_rank[&r];
        // Column width = max card width in this rank.
        let col_w: u16 = col.iter().map(|&n| size_of(n).0).max().unwrap_or(0);
        let mut y_cursor: u16 = 0;
        for &n in col {
            let (w, h) = size_of(n);
            let x_centred = x_cursor + col_w.saturating_sub(w) / 2;
            placements.push(NodePlacement {
                name: n.to_owned(),
                x: x_centred,
                y: y_cursor,
                w,
                h,
            });
            y_cursor = y_cursor.saturating_add(h).saturating_add(v_gap);
        }
        if y_cursor > max_y {
            max_y = y_cursor;
        }
        x_cursor = x_cursor.saturating_add(col_w).saturating_add(h_gap);
    }

    LayoutOut {
        nodes: placements,
        virtual_w: x_cursor.saturating_sub(h_gap),
        virtual_h: max_y.saturating_sub(v_gap),
    }
}

fn barycentre(n: &str, neigh: &HashMap<&str, Vec<&str>>, pos: &HashMap<&str, usize>) -> f64 {
    let ns = match neigh.get(n) {
        Some(v) if !v.is_empty() => v,
        _ => return pos.get(n).copied().unwrap_or(0) as f64,
    };
    // Use only neighbours that actually have a known position (they
    // always do, but be defensive against rank gaps).
    let mut sum = 0.0;
    let mut cnt = 0.0;
    for &m in ns {
        if let Some(&p) = pos.get(m) {
            sum += p as f64;
            cnt += 1.0;
        }
    }
    if cnt == 0.0 {
        pos.get(n).copied().unwrap_or(0) as f64
    } else {
        sum / cnt
    }
}

/// Convenience: dedup `nodes` preserving order. Useful for callers
/// that gather table names from multiple sources.
#[allow(dead_code)]
pub fn dedup_preserve_order(nodes: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        if seen.insert(n.clone()) {
            out.push(n.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_size(_: &str) -> (u16, u16) {
        (20, 5)
    }

    fn edge(a: &str, b: &str) -> RelationshipEdge {
        RelationshipEdge {
            from_table: a.to_owned(),
            from_columns: vec!["x".to_owned()],
            to_table: b.to_owned(),
            to_columns: vec!["id".to_owned()],
        }
    }

    #[test]
    fn empty_input_yields_empty_layout() {
        let out = layered_layout(&[], &[], fixed_size, 4, 1);
        assert!(out.nodes.is_empty());
        assert_eq!(out.virtual_w, 0);
        assert_eq!(out.virtual_h, 0);
    }

    #[test]
    fn linear_chain_ranks_left_to_right() {
        // a → b → c (b references a; c references b)
        let tables = vec!["a".into(), "b".into(), "c".into()];
        let edges = vec![edge("b", "a"), edge("c", "b")];
        let out = layered_layout(&tables, &edges, fixed_size, 4, 1);
        let a = out.get("a").unwrap();
        let b = out.get("b").unwrap();
        let c = out.get("c").unwrap();
        assert!(a.x < b.x, "a left of b: {} < {}", a.x, b.x);
        assert!(b.x < c.x, "b left of c: {} < {}", b.x, c.x);
    }

    #[test]
    fn parallel_roots_share_same_rank_stack_vertically() {
        // a and b are independent roots, both referenced by c.
        let tables = vec!["a".into(), "b".into(), "c".into()];
        let edges = vec![edge("c", "a"), edge("c", "b")];
        let out = layered_layout(&tables, &edges, fixed_size, 4, 1);
        let a = out.get("a").unwrap();
        let b = out.get("b").unwrap();
        let c = out.get("c").unwrap();
        assert_eq!(a.x, b.x, "roots share x: {} vs {}", a.x, b.x);
        assert_ne!(a.y, b.y, "roots stack vertically");
        assert!(c.x > a.x);
    }

    #[test]
    fn cycle_does_not_hang_and_places_every_node() {
        // a → b → a (cycle of length 2) plus orphan c
        let tables = vec!["a".into(), "b".into(), "c".into()];
        let edges = vec![edge("a", "b"), edge("b", "a")];
        let out = layered_layout(&tables, &edges, fixed_size, 4, 1);
        assert_eq!(out.nodes.len(), 3);
        assert!(out.get("a").is_some());
        assert!(out.get("b").is_some());
        assert!(out.get("c").is_some());
    }

    #[test]
    fn variable_card_sizes_are_respected() {
        let tables = vec!["small".into(), "tall".into()];
        let size = |n: &str| -> (u16, u16) {
            if n == "tall" {
                (30, 40)
            } else {
                (10, 3)
            }
        };
        // Same rank (both roots).
        let out = layered_layout(&tables, &[], size, 4, 2);
        let tall = out.get("tall").unwrap();
        let small = out.get("small").unwrap();
        assert_eq!(tall.h, 40);
        assert_eq!(small.h, 3);
        // virtual height >= tall + gap + small (or vice-versa) - gap.
        assert!(out.virtual_h >= 40 + 2 + 3, "got {}", out.virtual_h);
    }

    #[test]
    fn dedup_preserve_order_keeps_first_occurrence() {
        let v = vec!["b".into(), "a".into(), "b".into(), "c".into(), "a".into()];
        let out = dedup_preserve_order(&v);
        assert_eq!(out, vec!["b", "a", "c"]);
    }
}
