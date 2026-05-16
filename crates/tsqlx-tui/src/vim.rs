//! Vim-style modal editing for the SQL editor.
//!
//! Pure: takes a `KeyEvent` plus the current mode and a small piece
//! of pending state, returns the new mode + a sequence of `VimAction`s
//! the editor must execute. No `AppState`, no I/O.
//!
//! The editor owns the buffer/cursor and translates each `VimAction`
//! into mutations; this module is just the keymap.

#![allow(dead_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
    Visual,
}

impl VimMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
        }
    }
}

/// One unit of work for the editor to apply. `VimMode` transitions
/// are also expressed as actions so the caller has a single
/// homogenous queue to drain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimAction {
    EnterMode(VimMode),
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    MoveBufferStart,
    MoveBufferEnd,
    MoveWordForward,
    MoveWordBackward,
    InsertChar(char),
    InsertNewline,
    InsertLineBelow,
    InsertLineAbove,
    DeleteCharUnderCursor,
    DeleteCharBeforeCursor,
    DeleteLine,
    YankLine,
    DeleteSelection,
    YankSelection,
    Paste,
    StartCommandPalette,
    /// Backspace handling in Insert mode — the editor's existing
    /// backspace logic stays unchanged; this just signals it.
    BackspaceInsert,
}

/// Multi-key operator state. Vim's `dd`, `yy`, `gg` need to remember
/// the first half of the pair across two `handle_key` calls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingOp {
    /// Set to `Some('d')` after the user types `d` in Normal mode,
    /// waiting for the operand (only `d` itself is currently
    /// supported — `dd` deletes a line). `Some('y')` for `yy`.
    pub op: Option<char>,
    /// First `g` of `gg` was typed.
    pub g_pending: bool,
    /// Accumulated numeric prefix (vim's `5j`, `3dd`). Survives across
    /// keystrokes until an operator/motion consumes it or a non-count
    /// key resets it.
    pub count: Option<u32>,
}

impl PendingOp {
    pub fn reset(&mut self) {
        self.op = None;
        self.g_pending = false;
        self.count = None;
    }
}

/// Stateless key router. `pending` is the only piece of carryover
/// state; the editor owns it across calls.
pub fn handle_key(
    mode: VimMode,
    pending: &mut PendingOp,
    key: KeyEvent,
) -> (VimMode, Vec<VimAction>) {
    match mode {
        VimMode::Normal => handle_normal(pending, key),
        VimMode::Insert => handle_insert(key),
        VimMode::Visual => handle_visual(key),
    }
}

fn handle_normal(pending: &mut PendingOp, key: KeyEvent) -> (VimMode, Vec<VimAction>) {
    if pending.g_pending && matches!(key.code, KeyCode::Char('g')) {
        pending.reset();
        return (VimMode::Normal, vec![VimAction::MoveBufferStart]);
    }
    if pending.op == Some('d') && matches!(key.code, KeyCode::Char('d')) {
        let n = pending.count.take().unwrap_or(1).max(1);
        pending.reset();
        return (VimMode::Normal, repeat_action(VimAction::DeleteLine, n));
    }
    if pending.op == Some('y') && matches!(key.code, KeyCode::Char('y')) {
        let n = pending.count.take().unwrap_or(1).max(1);
        pending.reset();
        return (VimMode::Normal, repeat_action(VimAction::YankLine, n));
    }

    if let KeyCode::Char(c) = key.code {
        if c.is_ascii_digit() && !(c == '0' && pending.count.is_none()) {
            let digit = c.to_digit(10).unwrap_or(0);
            let next = pending
                .count
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit)
                .min(9_999);
            pending.count = Some(next);
            return (VimMode::Normal, vec![]);
        }
    }

    // Operator starters preserve any accumulated count so that
    // `3dd`/`3yy`/`3gg` can consume it on the second keystroke.
    match key.code {
        KeyCode::Char('d') => {
            pending.op = Some('d');
            pending.g_pending = false;
            return (VimMode::Normal, vec![]);
        }
        KeyCode::Char('y') => {
            pending.op = Some('y');
            pending.g_pending = false;
            return (VimMode::Normal, vec![]);
        }
        KeyCode::Char('g') => {
            pending.op = None;
            pending.g_pending = true;
            return (VimMode::Normal, vec![]);
        }
        _ => {}
    }

    let n = pending.count.take().unwrap_or(1).max(1);
    pending.op = None;
    pending.g_pending = false;

    match key.code {
        KeyCode::Esc => (VimMode::Normal, vec![]),
        KeyCode::Char('h') | KeyCode::Left => {
            (VimMode::Normal, repeat_action(VimAction::MoveLeft, n))
        }
        KeyCode::Char('j') | KeyCode::Down => {
            (VimMode::Normal, repeat_action(VimAction::MoveDown, n))
        }
        KeyCode::Char('k') | KeyCode::Up => (VimMode::Normal, repeat_action(VimAction::MoveUp, n)),
        KeyCode::Char('l') | KeyCode::Right => {
            (VimMode::Normal, repeat_action(VimAction::MoveRight, n))
        }
        KeyCode::Char('0') => (VimMode::Normal, vec![VimAction::MoveLineStart]),
        KeyCode::Char('$') => (VimMode::Normal, vec![VimAction::MoveLineEnd]),
        KeyCode::Char('w') => (
            VimMode::Normal,
            repeat_action(VimAction::MoveWordForward, n),
        ),
        KeyCode::Char('b') => (
            VimMode::Normal,
            repeat_action(VimAction::MoveWordBackward, n),
        ),
        // `NG` should jump to line N; v1 emits MoveBufferEnd once and drops the count.
        KeyCode::Char('G') => (VimMode::Normal, vec![VimAction::MoveBufferEnd]),
        KeyCode::Char('i') => (VimMode::Insert, vec![VimAction::EnterMode(VimMode::Insert)]),
        KeyCode::Char('a') => (
            VimMode::Insert,
            vec![VimAction::MoveRight, VimAction::EnterMode(VimMode::Insert)],
        ),
        KeyCode::Char('I') => (
            VimMode::Insert,
            vec![
                VimAction::MoveLineStart,
                VimAction::EnterMode(VimMode::Insert),
            ],
        ),
        KeyCode::Char('A') => (
            VimMode::Insert,
            vec![
                VimAction::MoveLineEnd,
                VimAction::EnterMode(VimMode::Insert),
            ],
        ),
        KeyCode::Char('o') => (
            VimMode::Insert,
            vec![
                VimAction::MoveLineEnd,
                VimAction::InsertLineBelow,
                VimAction::EnterMode(VimMode::Insert),
            ],
        ),
        KeyCode::Char('O') => (
            VimMode::Insert,
            vec![
                VimAction::MoveLineStart,
                VimAction::InsertLineAbove,
                VimAction::EnterMode(VimMode::Insert),
            ],
        ),
        KeyCode::Char('x') => (
            VimMode::Normal,
            repeat_action(VimAction::DeleteCharUnderCursor, n),
        ),
        KeyCode::Char('p') => (VimMode::Normal, repeat_action(VimAction::Paste, n)),
        KeyCode::Char('v') => (VimMode::Visual, vec![VimAction::EnterMode(VimMode::Visual)]),
        KeyCode::Char(':') => (VimMode::Normal, vec![VimAction::StartCommandPalette]),
        _ => (VimMode::Normal, vec![]),
    }
}

fn repeat_action(action: VimAction, n: u32) -> Vec<VimAction> {
    vec![action; n as usize]
}

fn handle_insert(key: KeyEvent) -> (VimMode, Vec<VimAction>) {
    match key.code {
        KeyCode::Esc => (VimMode::Normal, vec![VimAction::EnterMode(VimMode::Normal)]),
        KeyCode::Enter => (VimMode::Insert, vec![VimAction::InsertNewline]),
        KeyCode::Backspace => (VimMode::Insert, vec![VimAction::BackspaceInsert]),
        KeyCode::Left => (VimMode::Insert, vec![VimAction::MoveLeft]),
        KeyCode::Right => (VimMode::Insert, vec![VimAction::MoveRight]),
        KeyCode::Up => (VimMode::Insert, vec![VimAction::MoveUp]),
        KeyCode::Down => (VimMode::Insert, vec![VimAction::MoveDown]),
        KeyCode::Char(c) => {
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                (VimMode::Insert, vec![])
            } else {
                (VimMode::Insert, vec![VimAction::InsertChar(c)])
            }
        }
        _ => (VimMode::Insert, vec![]),
    }
}

fn handle_visual(key: KeyEvent) -> (VimMode, Vec<VimAction>) {
    match key.code {
        KeyCode::Esc => (VimMode::Normal, vec![VimAction::EnterMode(VimMode::Normal)]),
        KeyCode::Char('h') | KeyCode::Left => (VimMode::Visual, vec![VimAction::MoveLeft]),
        KeyCode::Char('j') | KeyCode::Down => (VimMode::Visual, vec![VimAction::MoveDown]),
        KeyCode::Char('k') | KeyCode::Up => (VimMode::Visual, vec![VimAction::MoveUp]),
        KeyCode::Char('l') | KeyCode::Right => (VimMode::Visual, vec![VimAction::MoveRight]),
        KeyCode::Char('0') => (VimMode::Visual, vec![VimAction::MoveLineStart]),
        KeyCode::Char('$') => (VimMode::Visual, vec![VimAction::MoveLineEnd]),
        KeyCode::Char('w') => (VimMode::Visual, vec![VimAction::MoveWordForward]),
        KeyCode::Char('b') => (VimMode::Visual, vec![VimAction::MoveWordBackward]),
        KeyCode::Char('y') => (
            VimMode::Normal,
            vec![
                VimAction::YankSelection,
                VimAction::EnterMode(VimMode::Normal),
            ],
        ),
        KeyCode::Char('d') => (
            VimMode::Normal,
            vec![
                VimAction::DeleteSelection,
                VimAction::EnterMode(VimMode::Normal),
            ],
        ),
        _ => (VimMode::Visual, vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn run(mode: VimMode, k: KeyEvent) -> (VimMode, Vec<VimAction>) {
        let mut p = PendingOp::default();
        handle_key(mode, &mut p, k)
    }

    #[test]
    fn normal_h_moves_left() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('h')));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::MoveLeft]);
    }

    #[test]
    fn normal_l_moves_right() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('l')));
        assert_eq!(a, vec![VimAction::MoveRight]);
    }

    #[test]
    fn normal_j_moves_down() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('j')));
        assert_eq!(a, vec![VimAction::MoveDown]);
    }

    #[test]
    fn normal_k_moves_up() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('k')));
        assert_eq!(a, vec![VimAction::MoveUp]);
    }

    #[test]
    fn normal_zero_moves_line_start() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('0')));
        assert_eq!(a, vec![VimAction::MoveLineStart]);
    }

    #[test]
    fn normal_dollar_moves_line_end() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('$')));
        assert_eq!(a, vec![VimAction::MoveLineEnd]);
    }

    #[test]
    fn normal_w_word_forward() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('w')));
        assert_eq!(a, vec![VimAction::MoveWordForward]);
    }

    #[test]
    fn normal_b_word_backward() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('b')));
        assert_eq!(a, vec![VimAction::MoveWordBackward]);
    }

    #[test]
    fn normal_i_enters_insert() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('i')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Insert)]);
    }

    #[test]
    fn normal_a_moves_right_then_insert() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('a')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![VimAction::MoveRight, VimAction::EnterMode(VimMode::Insert)]
        );
    }

    #[test]
    fn normal_cap_i() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('I')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::MoveLineStart,
                VimAction::EnterMode(VimMode::Insert)
            ]
        );
    }

    #[test]
    fn normal_cap_a() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('A')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::MoveLineEnd,
                VimAction::EnterMode(VimMode::Insert)
            ]
        );
    }

    #[test]
    fn normal_o_inserts_below() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('o')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::MoveLineEnd,
                VimAction::InsertLineBelow,
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
    }

    #[test]
    fn normal_cap_o_inserts_above() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('O')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::MoveLineStart,
                VimAction::InsertLineAbove,
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
    }

    #[test]
    fn normal_x_deletes_char() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('x')));
        assert_eq!(a, vec![VimAction::DeleteCharUnderCursor]);
    }

    #[test]
    fn normal_dd_deletes_line() {
        let mut p = PendingOp::default();
        let (_, a1) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('d')));
        assert!(a1.is_empty());
        assert_eq!(p.op, Some('d'));
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('d')));
        assert_eq!(a2, vec![VimAction::DeleteLine]);
        assert_eq!(p, PendingOp::default());
    }

    #[test]
    fn normal_yy_yanks_line() {
        let mut p = PendingOp::default();
        let (_, a1) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('y')));
        assert!(a1.is_empty());
        assert_eq!(p.op, Some('y'));
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('y')));
        assert_eq!(a2, vec![VimAction::YankLine]);
        assert_eq!(p, PendingOp::default());
    }

    #[test]
    fn normal_gg_buffer_start() {
        let mut p = PendingOp::default();
        let (_, a1) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('g')));
        assert!(a1.is_empty());
        assert!(p.g_pending);
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('g')));
        assert_eq!(a2, vec![VimAction::MoveBufferStart]);
        assert!(!p.g_pending);
    }

    #[test]
    fn normal_cap_g_buffer_end() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('G')));
        assert_eq!(a, vec![VimAction::MoveBufferEnd]);
    }

    #[test]
    fn normal_v_enters_visual() {
        let (m, a) = run(VimMode::Normal, key(KeyCode::Char('v')));
        assert_eq!(m, VimMode::Visual);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Visual)]);
    }

    #[test]
    fn normal_colon_starts_palette() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char(':')));
        assert_eq!(a, vec![VimAction::StartCommandPalette]);
    }

    #[test]
    fn insert_esc_returns_normal() {
        let (m, a) = run(VimMode::Insert, key(KeyCode::Esc));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Normal)]);
    }

    #[test]
    fn insert_char_emits_insert_char() {
        let (m, a) = run(VimMode::Insert, key(KeyCode::Char('a')));
        assert_eq!(m, VimMode::Insert);
        assert_eq!(a, vec![VimAction::InsertChar('a')]);
    }

    #[test]
    fn insert_enter_newline() {
        let (_, a) = run(VimMode::Insert, key(KeyCode::Enter));
        assert_eq!(a, vec![VimAction::InsertNewline]);
    }

    #[test]
    fn insert_backspace() {
        let (_, a) = run(VimMode::Insert, key(KeyCode::Backspace));
        assert_eq!(a, vec![VimAction::BackspaceInsert]);
    }

    #[test]
    fn insert_ctrl_char_is_empty() {
        let (m, a) = run(
            VimMode::Insert,
            key_mod(KeyCode::Char('r'), KeyModifiers::CONTROL),
        );
        assert_eq!(m, VimMode::Insert);
        assert!(a.is_empty());
    }

    #[test]
    fn insert_shift_char_emits_uppercase() {
        let (_, a) = run(
            VimMode::Insert,
            key_mod(KeyCode::Char('A'), KeyModifiers::SHIFT),
        );
        assert_eq!(a, vec![VimAction::InsertChar('A')]);
    }

    #[test]
    fn visual_y_yanks_and_exits() {
        let (m, a) = run(VimMode::Visual, key(KeyCode::Char('y')));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(
            a,
            vec![
                VimAction::YankSelection,
                VimAction::EnterMode(VimMode::Normal),
            ]
        );
    }

    #[test]
    fn visual_d_deletes_and_exits() {
        let (m, a) = run(VimMode::Visual, key(KeyCode::Char('d')));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(
            a,
            vec![
                VimAction::DeleteSelection,
                VimAction::EnterMode(VimMode::Normal),
            ]
        );
    }

    #[test]
    fn visual_esc_exits() {
        let (m, a) = run(VimMode::Visual, key(KeyCode::Esc));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Normal)]);
    }

    #[test]
    fn normal_d_then_j_drops_pending() {
        let mut p = PendingOp::default();
        let (_, a1) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('d')));
        assert!(a1.is_empty());
        assert_eq!(p.op, Some('d'));
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('j')));
        assert_eq!(a2, vec![VimAction::MoveDown]);
        assert_eq!(p.op, None);
    }

    fn feed(p: &mut PendingOp, mode: VimMode, keys: &[KeyCode]) -> (VimMode, Vec<VimAction>) {
        let mut last_mode = mode;
        let mut last_actions = vec![];
        for k in keys {
            let (m, a) = handle_key(last_mode, p, key(*k));
            last_mode = m;
            last_actions = a;
        }
        (last_mode, last_actions)
    }

    #[test]
    fn count_5j_repeats_movedown() {
        let mut p = PendingOp::default();
        let (_, a1) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('5')));
        assert!(a1.is_empty());
        assert_eq!(p.count, Some(5));
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('j')));
        assert_eq!(a2, vec![VimAction::MoveDown; 5]);
        assert_eq!(p.count, None);
    }

    #[test]
    fn count_10w_accumulates_digits() {
        let mut p = PendingOp::default();
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('1')));
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('0')));
        assert_eq!(p.count, Some(10));
        let (_, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('w')));
        assert_eq!(a, vec![VimAction::MoveWordForward; 10]);
    }

    #[test]
    fn count_2x_repeats_delete_char() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('2'), KeyCode::Char('x')],
        );
        assert_eq!(a, vec![VimAction::DeleteCharUnderCursor; 2]);
    }

    #[test]
    fn count_3dd_repeats_delete_line() {
        let mut p = PendingOp::default();
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('3')));
        assert_eq!(p.count, Some(3));
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('d')));
        assert_eq!(p.op, Some('d'));
        assert_eq!(p.count, Some(3));
        let (_, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('d')));
        assert_eq!(a, vec![VimAction::DeleteLine; 3]);
        assert_eq!(p, PendingOp::default());
    }

    #[test]
    fn count_3yy_repeats_yank_line() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('3'), KeyCode::Char('y'), KeyCode::Char('y')],
        );
        assert_eq!(a, vec![VimAction::YankLine; 3]);
        assert_eq!(p, PendingOp::default());
    }

    #[test]
    fn count_zero_alone_is_line_start() {
        let mut p = PendingOp::default();
        let (_, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('0')));
        assert_eq!(a, vec![VimAction::MoveLineStart]);
        assert_eq!(p.count, None);
    }

    #[test]
    fn count_30j_compound_digits() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('3'), KeyCode::Char('0'), KeyCode::Char('j')],
        );
        assert_eq!(a, vec![VimAction::MoveDown; 30]);
    }

    #[test]
    fn count_5i_discards_count_enters_insert() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('5'), KeyCode::Char('i')],
        );
        assert_eq!(m, VimMode::Insert);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Insert)]);
        assert_eq!(p.count, None);
    }

    #[test]
    fn count_5v_discards_count_enters_visual() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('5'), KeyCode::Char('v')],
        );
        assert_eq!(m, VimMode::Visual);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Visual)]);
        assert_eq!(p.count, None);
    }

    #[test]
    fn count_then_esc_resets() {
        let mut p = PendingOp::default();
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('3')));
        assert_eq!(p.count, Some(3));
        let (m, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Esc));
        assert_eq!(m, VimMode::Normal);
        assert!(a.is_empty());
        assert_eq!(p, PendingOp::default());
    }

    #[test]
    fn count_caps_at_9999() {
        let mut p = PendingOp::default();
        for _ in 0..8 {
            handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('9')));
        }
        assert_eq!(p.count, Some(9_999));
        let (_, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('j')));
        assert_eq!(a.len(), 9_999);
    }

    #[test]
    fn count_unrecognised_key_resets() {
        let mut p = PendingOp::default();
        handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('5')));
        assert_eq!(p.count, Some(5));
        let (_, a) = handle_key(VimMode::Normal, &mut p, key(KeyCode::F(1)));
        assert!(a.is_empty());
        assert_eq!(p.count, None);
        let (_, a2) = handle_key(VimMode::Normal, &mut p, key(KeyCode::Char('j')));
        assert_eq!(a2, vec![VimAction::MoveDown]);
    }

    #[test]
    fn count_3p_repeats_paste() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('3'), KeyCode::Char('p')],
        );
        assert_eq!(a, vec![VimAction::Paste; 3]);
    }

    #[test]
    fn insert_digit_inserts_char() {
        let (_, a) = run(VimMode::Insert, key(KeyCode::Char('5')));
        assert_eq!(a, vec![VimAction::InsertChar('5')]);
    }
}
