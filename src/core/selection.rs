//! Pane-local text selection model.
//!
//! This type is carried by [`crate::app::App`] and interpreted by
//! [`crate::platform::renderer`] for highlighting and clipboard copy behavior.

use super::layout::PaneId;

/// Tracks a text selection within a single pane.
///
/// Coordinates are pane-grid-local (column, row) relative to the inner area
/// (after borders). The `display_offset` snapshot captures the scrollback
/// position at selection start so extraction stays consistent even if new
/// output arrives.
#[derive(Debug, Clone)]
pub struct Selection {
    pub pane_id: PaneId,
    /// Start position (col, row) in pane-grid coords.
    pub start: (u16, u16),
    /// End position (col, row) in pane-grid coords.
    pub end: (u16, u16),
    /// The `display_offset` of the terminal at the time selection started.
    pub display_offset: usize,
}

impl Selection {
    /// Normalise so that start <= end in reading order (top-left to bottom-right).
    ///
    /// # Examples
    ///
    /// ```text
    /// // Dragging backward still yields an ordered range.
    /// let ((sc, sr), (ec, er)) = selection.ordered();
    /// ```
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let (sc, sr) = self.start;
        let (ec, er) = self.end;
        if sr < er || (sr == er && sc <= ec) {
            ((sc, sr), (ec, er))
        } else {
            ((ec, er), (sc, sr))
        }
    }

    /// True when start and end are the same cell (a click, not a drag).
    ///
    /// This is used to distinguish clicks from actual selection drags.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}
