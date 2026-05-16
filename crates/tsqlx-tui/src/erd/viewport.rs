//! Viewport state for the whole-schema canvas: a window onto a
//! virtual grid larger than the screen. Pure state + math.

/// Card density. Picked when entering canvas view and cycled with
/// `+` / `-`. Drives the per-card width/height the layout sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zoom {
    /// Just the table name. One cell tall (plus borders).
    Collapsed,
    /// Name + PK + FK columns. The default landing zoom.
    #[default]
    Compact,
    /// Every column with its data type.
    Full,
}

impl Zoom {
    /// `+` key: zoom in (more detail).
    pub fn zoom_in(self) -> Self {
        match self {
            Self::Collapsed => Self::Compact,
            Self::Compact | Self::Full => Self::Full,
        }
    }
    /// `-` key: zoom out (less detail).
    pub fn zoom_out(self) -> Self {
        match self {
            Self::Full => Self::Compact,
            Self::Compact | Self::Collapsed => Self::Collapsed,
        }
    }
}

/// Viewport position over the virtual canvas. `(0, 0)` shows the
/// top-left cell.
#[derive(Debug, Clone, Copy, Default)]
pub struct Viewport {
    pub offset_x: u16,
    pub offset_y: u16,
    pub zoom: Zoom,
    /// `(virtual_x, virtual_y)` of the cell that was under the mouse
    /// when the drag started, so we can pan by `(drag - current)`.
    pub drag_anchor: Option<(u16, u16)>,
}

impl Viewport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-clamp offset so the viewport never starts past the right
    /// or bottom edge of the virtual canvas. Pass the actual viewport
    /// pane size (in cells) and the virtual canvas size.
    pub fn clamp(&mut self, virt_w: u16, virt_h: u16, view_w: u16, view_h: u16) {
        let max_x = virt_w.saturating_sub(view_w);
        let max_y = virt_h.saturating_sub(view_h);
        self.offset_x = self.offset_x.min(max_x);
        self.offset_y = self.offset_y.min(max_y);
    }

    /// Pan by a signed delta in cell units (positive = right / down).
    /// Saturates at zero. Caller should `clamp` afterwards.
    pub fn pan(&mut self, dx: i32, dy: i32) {
        self.offset_x = saturating_add_signed(self.offset_x, dx);
        self.offset_y = saturating_add_signed(self.offset_y, dy);
    }

    /// Centre on a `(virt_x, virt_y)` cell of size `(card_w, card_h)`
    /// within a viewport of `(view_w, view_h)`. Caller should `clamp`
    /// afterwards. Reserved for the upcoming `c` recentre-on-selected
    /// key path.
    #[allow(dead_code)]
    pub fn centre_on(
        &mut self,
        virt_x: u16,
        virt_y: u16,
        card_w: u16,
        card_h: u16,
        view_w: u16,
        view_h: u16,
    ) {
        let target_x = virt_x as i32 + card_w as i32 / 2 - view_w as i32 / 2;
        let target_y = virt_y as i32 + card_h as i32 / 2 - view_h as i32 / 2;
        self.offset_x = target_x.max(0) as u16;
        self.offset_y = target_y.max(0) as u16;
    }

    /// Begin a drag at `(view_x, view_y)` in the viewport pane.
    pub fn drag_begin(&mut self, view_x: u16, view_y: u16) {
        self.drag_anchor = Some((
            self.offset_x.saturating_add(view_x),
            self.offset_y.saturating_add(view_y),
        ));
    }

    /// Continue a drag: the cell under the mouse should remain the
    /// anchor cell. Returns whether the offset actually moved (so the
    /// caller can decide whether to repaint).
    pub fn drag_update(&mut self, view_x: u16, view_y: u16) -> bool {
        let Some((ax, ay)) = self.drag_anchor else {
            return false;
        };
        let new_off_x = ax.saturating_sub(view_x);
        let new_off_y = ay.saturating_sub(view_y);
        let changed = new_off_x != self.offset_x || new_off_y != self.offset_y;
        self.offset_x = new_off_x;
        self.offset_y = new_off_y;
        changed
    }

    pub fn drag_end(&mut self) {
        self.drag_anchor = None;
    }
}

fn saturating_add_signed(base: u16, delta: i32) -> u16 {
    if delta >= 0 {
        base.saturating_add(delta as u16)
    } else {
        base.saturating_sub((-delta) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_in_and_out_cycle_through_three_levels() {
        assert_eq!(Zoom::default(), Zoom::Compact);
        assert_eq!(Zoom::Compact.zoom_in(), Zoom::Full);
        assert_eq!(Zoom::Full.zoom_in(), Zoom::Full); // saturate
        assert_eq!(Zoom::Compact.zoom_out(), Zoom::Collapsed);
        assert_eq!(Zoom::Collapsed.zoom_out(), Zoom::Collapsed); // saturate
        assert_eq!(Zoom::Collapsed.zoom_in(), Zoom::Compact);
    }

    #[test]
    fn pan_saturates_at_zero() {
        let mut v = Viewport::new();
        v.pan(5, 7);
        assert_eq!((v.offset_x, v.offset_y), (5, 7));
        v.pan(-100, -100);
        assert_eq!((v.offset_x, v.offset_y), (0, 0));
    }

    #[test]
    fn clamp_caps_at_virtual_minus_view() {
        let mut v = Viewport::new();
        v.pan(500, 500);
        v.clamp(200, 100, 80, 24);
        assert_eq!(v.offset_x, 200 - 80);
        assert_eq!(v.offset_y, 100 - 24);
    }

    #[test]
    fn clamp_zero_when_virtual_smaller_than_view() {
        let mut v = Viewport::new();
        v.pan(50, 50);
        v.clamp(40, 10, 80, 24);
        assert_eq!((v.offset_x, v.offset_y), (0, 0));
    }

    #[test]
    fn centre_on_places_card_in_middle_of_viewport() {
        let mut v = Viewport::new();
        // Card at virtual (100, 50), 20×5. Viewport 80×24.
        // Want offset so that card centre lands at viewport centre.
        v.centre_on(100, 50, 20, 5, 80, 24);
        // Card centre = (110, 52). View centre = (40, 12). offset = (70, 40).
        assert_eq!(v.offset_x, 70);
        assert_eq!(v.offset_y, 40);
    }

    #[test]
    fn drag_keeps_anchor_cell_under_cursor() {
        let mut v = Viewport::new();
        v.pan(10, 5);
        // Cursor at (3, 4) → anchor at virtual (13, 9).
        v.drag_begin(3, 4);
        assert_eq!(v.drag_anchor, Some((13, 9)));
        // Cursor moves to (5, 6) → offset becomes (13-5, 9-6) = (8, 3).
        let moved = v.drag_update(5, 6);
        assert!(moved);
        assert_eq!((v.offset_x, v.offset_y), (8, 3));
        // No-op update: same cursor pos → no change reported.
        let moved2 = v.drag_update(5, 6);
        assert!(!moved2);
        v.drag_end();
        assert_eq!(v.drag_anchor, None);
    }

    #[test]
    fn drag_update_without_begin_is_a_noop() {
        let mut v = Viewport::new();
        let moved = v.drag_update(10, 10);
        assert!(!moved);
        assert_eq!((v.offset_x, v.offset_y), (0, 0));
    }
}
