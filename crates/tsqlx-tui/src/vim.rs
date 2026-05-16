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
    /// Jump to the 1-based line index `n` (vim's `Ngg` / `NG`). The
    /// editor clamps to the last line if `n` is past the end.
    MoveToLine(u32),
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
    /// Undo the previous edit. Bound to `u` in Normal mode.
    Undo,
    /// Redo a previously-undone edit. Bound to `U` in Normal mode
    /// (vim's `Ctrl+R` is reserved by tsqlx for "run all queries").
    Redo,
    /// Set the register the NEXT `YankLine` / `DeleteLine` /
    /// `YankSelection` / `DeleteSelection` / `Paste` should use.
    /// Lib.rs holds the live register map and clears the override
    /// after the consuming action.
    RegisterOverride(char),
    /// Start recording inserted characters; the editor will replay
    /// them `replays` additional times when the matching
    /// `EndInsertReplay` arrives. Emitted by `Ni` / `Na` / `NI` /
    /// `NA` / `No` / `NO` with count >= 2.
    BeginInsertReplay(u32),
    /// Stop recording and trigger any pending replay. Emitted on
    /// the Insert → Normal transition.
    EndInsertReplay,
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
    /// `"` was typed; the next key is interpreted as a register
    /// letter rather than a normal command.
    pub awaiting_register: bool,
}

impl PendingOp {
    pub fn reset(&mut self) {
        self.op = None;
        self.g_pending = false;
        self.count = None;
        self.awaiting_register = false;
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
    // `"<letter>` — register selection. The first arm consumes the
    // letter that follows the `"`; the second arm sets up the wait.
    if pending.awaiting_register {
        pending.awaiting_register = false;
        if let KeyCode::Char(c) = key.code {
            if c.is_ascii_alphabetic() {
                let reg = c.to_ascii_lowercase();
                return (VimMode::Normal, vec![VimAction::RegisterOverride(reg)]);
            }
        }
        // Non-letter after `"` — drop the register selection and
        // fall through to interpret the key normally. Done by
        // re-entering this function with a fresh state path.
    }

    if pending.g_pending && matches!(key.code, KeyCode::Char('g')) {
        // `Ngg` jumps to line N (1-based). Without a count, gg goes
        // to the buffer start (= line 1, but MoveBufferStart also
        // sets cursor to byte 0 which is semantically identical).
        let n = pending.count.take();
        pending.reset();
        let action = match n {
            Some(line) if line >= 1 => VimAction::MoveToLine(line),
            _ => VimAction::MoveBufferStart,
        };
        return (VimMode::Normal, vec![action]);
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
        KeyCode::Char('"') => {
            pending.awaiting_register = true;
            return (VimMode::Normal, vec![]);
        }
        _ => {}
    }

    let n = pending.count.take().unwrap_or(1).max(1);
    pending.op = None;
    pending.g_pending = false;

    // Helper: build the action list for entering Insert mode, with
    // an optional `BeginInsertReplay` prefix when the user gave a
    // count >= 2. `n` is the OUTER repeat count (`5i` => replay 4
    // more times after the original keystrokes).
    let enter_insert_with_replay = |mut prefix: Vec<VimAction>| -> Vec<VimAction> {
        let mut out: Vec<VimAction> = Vec::with_capacity(prefix.len() + 2);
        if n > 1 {
            out.push(VimAction::BeginInsertReplay(n - 1));
        }
        out.append(&mut prefix);
        out.push(VimAction::EnterMode(VimMode::Insert));
        out
    };

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
        // `NG` jumps to line N (1-based); `G` alone goes to buffer end.
        KeyCode::Char('G') => {
            if n > 1 {
                (VimMode::Normal, vec![VimAction::MoveToLine(n)])
            } else {
                (VimMode::Normal, vec![VimAction::MoveBufferEnd])
            }
        }
        KeyCode::Char('i') => (VimMode::Insert, enter_insert_with_replay(vec![])),
        KeyCode::Char('a') => (
            VimMode::Insert,
            enter_insert_with_replay(vec![VimAction::MoveRight]),
        ),
        KeyCode::Char('I') => (
            VimMode::Insert,
            enter_insert_with_replay(vec![VimAction::MoveLineStart]),
        ),
        KeyCode::Char('A') => (
            VimMode::Insert,
            enter_insert_with_replay(vec![VimAction::MoveLineEnd]),
        ),
        KeyCode::Char('o') => (
            VimMode::Insert,
            enter_insert_with_replay(vec![VimAction::MoveLineEnd, VimAction::InsertLineBelow]),
        ),
        KeyCode::Char('O') => (
            VimMode::Insert,
            enter_insert_with_replay(vec![VimAction::MoveLineStart, VimAction::InsertLineAbove]),
        ),
        KeyCode::Char('x') => (
            VimMode::Normal,
            repeat_action(VimAction::DeleteCharUnderCursor, n),
        ),
        KeyCode::Char('p') => (VimMode::Normal, repeat_action(VimAction::Paste, n)),
        KeyCode::Char('u') => (VimMode::Normal, vec![VimAction::Undo]),
        KeyCode::Char('U') => (VimMode::Normal, vec![VimAction::Redo]),
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
        KeyCode::Esc => (
            VimMode::Normal,
            vec![
                VimAction::EndInsertReplay,
                VimAction::EnterMode(VimMode::Normal),
            ],
        ),
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
        // Esc-out-of-Insert emits the replay terminator first so the
        // editor can apply any pending `Ni` replay, then transitions
        // back to Normal.
        let (m, a) = run(VimMode::Insert, key(KeyCode::Esc));
        assert_eq!(m, VimMode::Normal);
        assert_eq!(
            a,
            vec![
                VimAction::EndInsertReplay,
                VimAction::EnterMode(VimMode::Normal),
            ]
        );
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
    fn count_5i_emits_begin_insert_replay() {
        // `5i` now starts a 4-replay session — vim's `Ni` semantics.
        // Previously the count was silently dropped; the new
        // behaviour wraps the insert with BeginInsertReplay(n - 1)
        // so the editor can re-emit the typed text n - 1 more times
        // on Esc.
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('5'), KeyCode::Char('i')],
        );
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::BeginInsertReplay(4),
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
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

    // ── new in this PR: jump-to-line, registers, undo/redo, Ni replay ──

    #[test]
    fn count_5g_jumps_to_line_5() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('5'), KeyCode::Char('G')],
        );
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::MoveToLine(5)]);
    }

    #[test]
    fn capital_g_alone_goes_to_buffer_end() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('G')));
        assert_eq!(a, vec![VimAction::MoveBufferEnd]);
    }

    #[test]
    fn count_7gg_jumps_to_line_7() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('7'), KeyCode::Char('g'), KeyCode::Char('g')],
        );
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::MoveToLine(7)]);
    }

    #[test]
    fn double_g_without_count_goes_to_buffer_start() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('g'), KeyCode::Char('g')],
        );
        assert_eq!(a, vec![VimAction::MoveBufferStart]);
    }

    #[test]
    fn quote_a_emits_register_override_lowercase() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('"'), KeyCode::Char('a')],
        );
        assert_eq!(m, VimMode::Normal);
        assert_eq!(a, vec![VimAction::RegisterOverride('a')]);
        assert!(!p.awaiting_register);
    }

    #[test]
    fn quote_uppercase_z_lowercases_to_z() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('"'), KeyCode::Char('Z')],
        );
        assert_eq!(a, vec![VimAction::RegisterOverride('z')]);
    }

    #[test]
    fn quote_then_non_letter_drops_selection_silently() {
        let mut p = PendingOp::default();
        // After `"`, `5` is not a register letter — wait flag clears
        // and the `5` falls through to the digit branch, becoming
        // the start of a count.
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('"'), KeyCode::Char('5')],
        );
        assert_eq!(a, Vec::<VimAction>::new());
        assert!(!p.awaiting_register);
        assert_eq!(p.count, Some(5));
    }

    #[test]
    fn quote_a_then_yy_yanks_into_register_a() {
        // Sequence emits: RegisterOverride('a'), then YankLine.
        // The editor (not this module) consumes the override when
        // applying the YankLine action.
        let mut p = PendingOp::default();
        let (_, mut a) = feed(
            &mut p,
            VimMode::Normal,
            &[
                KeyCode::Char('"'),
                KeyCode::Char('a'),
                KeyCode::Char('y'),
                KeyCode::Char('y'),
            ],
        );
        // The last yy emits YankLine — concat earlier
        // RegisterOverride('a') emitted by the second key.
        let yy = a.pop().unwrap();
        assert_eq!(yy, VimAction::YankLine);
    }

    #[test]
    fn normal_u_emits_undo() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('u')));
        assert_eq!(a, vec![VimAction::Undo]);
    }

    #[test]
    fn normal_capital_u_emits_redo() {
        let (_, a) = run(VimMode::Normal, key(KeyCode::Char('U')));
        assert_eq!(a, vec![VimAction::Redo]);
    }

    #[test]
    fn count_3i_emits_replay_2() {
        // 3 keystrokes → replay count = 2 (3 total instances).
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('3'), KeyCode::Char('i')],
        );
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::BeginInsertReplay(2),
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
    }

    #[test]
    fn count_5o_emits_replay_then_open_below() {
        let mut p = PendingOp::default();
        let (m, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('5'), KeyCode::Char('o')],
        );
        assert_eq!(m, VimMode::Insert);
        assert_eq!(
            a,
            vec![
                VimAction::BeginInsertReplay(4),
                VimAction::MoveLineEnd,
                VimAction::InsertLineBelow,
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
    }

    #[test]
    fn count_2a_emits_replay_one_plus_moveright() {
        let mut p = PendingOp::default();
        let (_, a) = feed(
            &mut p,
            VimMode::Normal,
            &[KeyCode::Char('2'), KeyCode::Char('a')],
        );
        assert_eq!(
            a,
            vec![
                VimAction::BeginInsertReplay(1),
                VimAction::MoveRight,
                VimAction::EnterMode(VimMode::Insert),
            ]
        );
    }

    #[test]
    fn plain_i_without_count_does_not_emit_replay() {
        let mut p = PendingOp::default();
        let (m, a) = feed(&mut p, VimMode::Normal, &[KeyCode::Char('i')]);
        assert_eq!(m, VimMode::Insert);
        assert_eq!(a, vec![VimAction::EnterMode(VimMode::Insert)]);
    }
}
