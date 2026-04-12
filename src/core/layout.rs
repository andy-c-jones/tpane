use std::collections::HashMap;

/// Unique identifier for a leaf pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// Split axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,   // left | right
    Horizontal, // top / bottom
}

/// Pure layout tree — no runtime state.
#[derive(Debug, Clone)]
pub enum Node {
    Leaf(PaneId),
    Split {
        orientation: Orientation,
        /// 0.0..=1.0 fraction given to the first child.
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// Resolved geometry for a pane.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Where the new pane is placed relative to the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitPosition {
    /// New pane becomes the first child (left or above).
    Before,
    /// New pane becomes the second child (right or below).
    After,
}

/// The full layout state.
pub struct Layout {
    pub root: Node,
    pub active: PaneId,
    next_id: u32,
    /// Stack of previously focused pane IDs (most recent last).
    focus_history: Vec<PaneId>,
}

impl Layout {
    pub fn new() -> Self {
        let root_id = PaneId(0);
        Self { root: Node::Leaf(root_id), active: root_id, next_id: 1, focus_history: Vec::new() }
    }

    pub fn next_id(&mut self) -> PaneId {
        let id = PaneId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Split the active pane (new pane goes after/right/below), returning the new pane's ID.
    #[allow(dead_code)]
    pub fn split(&mut self, orientation: Orientation) -> PaneId {
        self.split_with_position(orientation, SplitPosition::After)
    }

    /// Split the active pane with explicit positioning of the new pane.
    pub fn split_with_position(&mut self, orientation: Orientation, position: SplitPosition) -> PaneId {
        let new_id = self.next_id();
        let target = self.active;
        split_node(&mut self.root, target, orientation, new_id, position);
        self.focus_history.push(self.active);
        self.active = new_id;
        new_id
    }

    /// Close the active pane. Returns false if it is the last pane.
    #[allow(dead_code)]
    pub fn close_active(&mut self) -> bool {
        self.close_pane(self.active)
    }

    /// Close a specific pane by ID. Returns false if it is the last pane.
    pub fn close_pane(&mut self, id: PaneId) -> bool {
        let leaves: Vec<PaneId> = collect_leaves(&self.root);
        if leaves.len() <= 1 {
            return false;
        }

        if remove_leaf(&mut self.root, id) {
            // Remove closed pane from focus history.
            self.focus_history.retain(|&pid| pid != id);

            if self.active == id {
                let remaining: Vec<PaneId> = collect_leaves(&self.root);
                // Pop the most recent still-alive pane from history.
                let next = loop {
                    match self.focus_history.pop() {
                        Some(prev) if remaining.contains(&prev) => break prev,
                        Some(_) => continue, // stale entry, skip
                        None => break remaining[0], // fallback to first leaf
                    }
                };
                self.active = next;
            }
            return true;
        }
        false
    }

    /// Compute pixel/cell geometry for every pane given terminal dimensions.
    pub fn compute_rects(&self, width: u16, height: u16) -> HashMap<PaneId, Rect> {
        let mut map = HashMap::new();
        compute(&self.root, Rect { x: 0, y: 0, width, height }, &mut map);
        map
    }

    /// Cycle focus to the next leaf in document order.
    pub fn focus_next(&mut self) {
        let leaves = collect_leaves(&self.root);
        if let Some(pos) = leaves.iter().position(|&id| id == self.active) {
            self.focus_history.push(self.active);
            self.active = leaves[(pos + 1) % leaves.len()];
        }
    }

    /// Cycle focus to the previous leaf.
    pub fn focus_prev(&mut self) {
        let leaves = collect_leaves(&self.root);
        if let Some(pos) = leaves.iter().position(|&id| id == self.active) {
            let len = leaves.len();
            self.focus_history.push(self.active);
            self.active = leaves[(pos + len - 1) % len];
        }
    }

    #[allow(dead_code)]
    pub fn leaf_ids(&self) -> Vec<PaneId> {
        collect_leaves(&self.root)
    }

    /// Set the active pane, recording the current active in focus history.
    pub fn set_active(&mut self, id: PaneId) {
        if id != self.active {
            self.focus_history.push(self.active);
            self.active = id;
        }
    }

    /// Focus the nearest pane in the given direction using spatial geometry.
    /// `terminal_size` is (width, height) used to compute pane rects.
    pub fn focus_direction(&mut self, direction: Direction, terminal_size: (u16, u16)) {
        let (w, h) = terminal_size;
        let rects = self.compute_rects(w, h);

        let active_rect = match rects.get(&self.active) {
            Some(r) => r,
            None => return,
        };

        // Centre of the active pane.
        let ax = active_rect.x as i32 + active_rect.width as i32 / 2;
        let ay = active_rect.y as i32 + active_rect.height as i32 / 2;

        let mut best: Option<(PaneId, i32)> = None;

        for (&id, rect) in &rects {
            if id == self.active {
                continue;
            }

            let cx = rect.x as i32 + rect.width as i32 / 2;
            let cy = rect.y as i32 + rect.height as i32 / 2;

            // Check if the candidate is in the correct direction.
            let in_direction = match direction {
                Direction::Left  => cx < ax,
                Direction::Right => cx > ax,
                Direction::Up    => cy < ay,
                Direction::Down  => cy > ay,
            };
            if !in_direction {
                continue;
            }

            // Distance metric: Manhattan distance, weighted so that the primary
            // axis (the direction of travel) matters more than the cross axis.
            let dist = match direction {
                Direction::Left | Direction::Right => {
                    (cx - ax).abs() + (cy - ay).abs() * 2
                }
                Direction::Up | Direction::Down => {
                    (cy - ay).abs() + (cx - ax).abs() * 2
                }
            };

            if best.is_none() || dist < best.unwrap().1 {
                best = Some((id, dist));
            }
        }

        if let Some((id, _)) = best {
            self.focus_history.push(self.active);
            self.active = id;
        }
    }
}

/// Direction for spatial pane navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

// ── helpers ────────────────────────────────────────────────────────────────

fn collect_leaves(node: &Node) -> Vec<PaneId> {
    match node {
        Node::Leaf(id) => vec![*id],
        Node::Split { first, second, .. } => {
            let mut v = collect_leaves(first);
            v.extend(collect_leaves(second));
            v
        }
    }
}

fn split_node(node: &mut Node, target: PaneId, orientation: Orientation, new_id: PaneId, position: SplitPosition) -> bool {
    match node {
        Node::Leaf(id) if *id == target => {
            let old_leaf = Node::Leaf(*id);
            let new_leaf = Node::Leaf(new_id);
            let (first, second) = match position {
                SplitPosition::Before => (new_leaf, old_leaf),
                SplitPosition::After => (old_leaf, new_leaf),
            };
            *node = Node::Split {
                orientation,
                ratio: 0.5,
                first: Box::new(first),
                second: Box::new(second),
            };
            true
        }
        Node::Leaf(_) => false,
        Node::Split { first, second, .. } => {
            split_node(first, target, orientation, new_id, position)
                || split_node(second, target, orientation, new_id, position)
        }
    }
}

/// Remove `target` leaf and replace its parent split with the sibling.
/// Returns true if the tree was mutated.
fn remove_leaf(node: &mut Node, target: PaneId) -> bool {
    let should_replace = match node {
        Node::Split { first, second, .. } => {
            if matches!(first.as_ref(), Node::Leaf(id) if *id == target) {
                Some(std::mem::replace(second.as_mut(), Node::Leaf(PaneId(0))))
            } else if matches!(second.as_ref(), Node::Leaf(id) if *id == target) {
                Some(std::mem::replace(first.as_mut(), Node::Leaf(PaneId(0))))
            } else {
                None
            }
        }
        _ => None,
    };

    if let Some(replacement) = should_replace {
        *node = replacement;
        return true;
    }

    match node {
        Node::Split { first, second, .. } => {
            remove_leaf(first, target) || remove_leaf(second, target)
        }
        _ => false,
    }
}

fn compute(node: &Node, rect: Rect, map: &mut HashMap<PaneId, Rect>) {
    match node {
        Node::Leaf(id) => {
            map.insert(*id, rect);
        }
        Node::Split { orientation, ratio, first, second } => {
            let (r1, r2) = split_rect(rect, *orientation, *ratio);
            compute(first, r1, map);
            compute(second, r2, map);
        }
    }
}

fn split_rect(rect: Rect, orientation: Orientation, ratio: f64) -> (Rect, Rect) {
    match orientation {
        Orientation::Vertical => {
            let first_w = ((rect.width as f64 * ratio) as u16).max(1);
            // Skip the divider column if there's no room for it.
            let divider = if rect.width > first_w + 1 { 1 } else { 0 };
            let second_x = (rect.x + first_w + divider)
                .min(rect.x + rect.width.saturating_sub(1));
            let second_w = (rect.x + rect.width).saturating_sub(second_x).max(1);
            (
                Rect { x: rect.x, y: rect.y, width: first_w, height: rect.height },
                Rect { x: second_x, y: rect.y, width: second_w, height: rect.height },
            )
        }
        Orientation::Horizontal => {
            let first_h = ((rect.height as f64 * ratio) as u16).max(1);
            let divider = if rect.height > first_h + 1 { 1 } else { 0 };
            let second_y = (rect.y + first_h + divider)
                .min(rect.y + rect.height.saturating_sub(1));
            let second_h = (rect.y + rect.height).saturating_sub(second_y).max(1);
            (
                Rect { x: rect.x, y: rect.y, width: rect.width, height: first_h },
                Rect { x: rect.x, y: second_y, width: rect.width, height: second_h },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── geometry helpers ──────────────────────────────────────────────────────

    fn no_overlap(rects: &HashMap<PaneId, Rect>) {
        let list: Vec<Rect> = rects.values().copied().collect();
        for (i, a) in list.iter().enumerate() {
            for b in list.iter().skip(i + 1) {
                let x_overlap = a.x < b.x + b.width && b.x < a.x + a.width;
                let y_overlap = a.y < b.y + b.height && b.y < a.y + a.height;
                assert!(!(x_overlap && y_overlap), "overlap between {:?} and {:?}", a, b);
            }
        }
    }

    fn all_in_bounds(rects: &HashMap<PaneId, Rect>, w: u16, h: u16) {
        for r in rects.values() {
            assert!(r.x + r.width <= w, "rect {:?} exceeds width {w}", r);
            assert!(r.y + r.height <= h, "rect {:?} exceeds height {h}", r);
        }
    }

    // ── single pane ───────────────────────────────────────────────────────────

    #[test]
    fn single_pane_occupies_full_area() {
        let layout = Layout::new();
        let rects = layout.compute_rects(100, 40);
        let r = rects[&PaneId(0)];
        assert_eq!(r.width, 100);
        assert_eq!(r.height, 40);
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 0);
    }

    // ── vertical split ────────────────────────────────────────────────────────

    #[test]
    fn vertical_split_divides_width() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        let rects = layout.compute_rects(100, 40);
        let w: u16 = rects.values().map(|r| r.width).sum::<u16>() + 1;
        assert_eq!(w, 100);
    }

    #[test]
    fn vertical_split_preserves_full_height() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        let rects = layout.compute_rects(80, 24);
        for r in rects.values() {
            assert_eq!(r.height, 24);
        }
    }

    #[test]
    fn vertical_split_no_overlap() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        let rects = layout.compute_rects(80, 24);
        no_overlap(&rects);
        all_in_bounds(&rects, 80, 24);
    }

    // ── horizontal split ──────────────────────────────────────────────────────

    #[test]
    fn horizontal_split_divides_height() {
        let mut layout = Layout::new();
        layout.split(Orientation::Horizontal);
        let rects = layout.compute_rects(80, 24);
        let h: u16 = rects.values().map(|r| r.height).sum::<u16>() + 1;
        assert_eq!(h, 24);
    }

    #[test]
    fn horizontal_split_preserves_full_width() {
        let mut layout = Layout::new();
        layout.split(Orientation::Horizontal);
        let rects = layout.compute_rects(80, 24);
        for r in rects.values() {
            assert_eq!(r.width, 80);
        }
    }

    #[test]
    fn horizontal_split_no_overlap() {
        let mut layout = Layout::new();
        layout.split(Orientation::Horizontal);
        let rects = layout.compute_rects(80, 24);
        no_overlap(&rects);
        all_in_bounds(&rects, 80, 24);
    }

    // ── tiny terminal sizes ───────────────────────────────────────────────────

    #[test]
    fn tiny_terminal_splits_stay_in_bounds() {
        for w in [1u16, 2, 3, 4] {
            for h in [1u16, 2, 3, 4] {
                let mut layout = Layout::new();
                layout.split(Orientation::Vertical);
                let rects = layout.compute_rects(w, h);
                all_in_bounds(&rects, w, h);
                if w >= 2 { no_overlap(&rects); }

                let mut layout = Layout::new();
                layout.split(Orientation::Horizontal);
                let rects = layout.compute_rects(w, h);
                all_in_bounds(&rects, w, h);
                if h >= 2 { no_overlap(&rects); }
            }
        }
    }

    // ── multiple splits ───────────────────────────────────────────────────────

    #[test]
    fn three_pane_split_no_overlap() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        layout.split(Orientation::Horizontal);
        let rects = layout.compute_rects(120, 40);
        assert_eq!(rects.len(), 3);
        no_overlap(&rects);
        all_in_bounds(&rects, 120, 40);
    }

    #[test]
    fn split_makes_new_pane_active() {
        let mut layout = Layout::new();
        let original = layout.active;
        let new_id = layout.split(Orientation::Vertical);
        assert_ne!(new_id, original);
        assert_eq!(layout.active, new_id);
    }

    // ── close ─────────────────────────────────────────────────────────────────

    #[test]
    fn close_last_pane_is_noop() {
        let mut layout = Layout::new();
        assert!(!layout.close_active());
        assert_eq!(layout.leaf_ids().len(), 1);
    }

    #[test]
    fn close_active_removes_split() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        assert_eq!(layout.leaf_ids().len(), 2);
        assert!(layout.close_active());
        assert_eq!(layout.leaf_ids().len(), 1);
    }

    #[test]
    fn close_pane_by_id_removes_non_active() {
        let mut layout = Layout::new();
        let first = layout.active;
        let _second = layout.split(Orientation::Vertical);
        // active is second; close first by id
        assert!(layout.close_pane(first));
        assert_eq!(layout.leaf_ids().len(), 1);
        assert!(!layout.leaf_ids().contains(&first));
    }

    #[test]
    fn close_active_pane_shifts_focus() {
        let mut layout = Layout::new();
        let first = layout.active;
        layout.split(Orientation::Vertical);
        let active_after_split = layout.active;
        layout.close_active();
        // focus moves to remaining pane (first)
        assert_eq!(layout.active, first);
        assert!(!layout.leaf_ids().contains(&active_after_split));
    }

    // ── focus ─────────────────────────────────────────────────────────────────

    #[test]
    fn focus_next_cycles() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        let ids = layout.leaf_ids();
        layout.active = ids[0];
        layout.focus_next();
        assert_eq!(layout.active, ids[1]);
        layout.focus_next(); // wraps back
        assert_eq!(layout.active, ids[0]);
    }

    #[test]
    fn focus_prev_cycles() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        let ids = layout.leaf_ids();
        layout.active = ids[1];
        layout.focus_prev();
        assert_eq!(layout.active, ids[0]);
        layout.focus_prev(); // wraps back
        assert_eq!(layout.active, ids[1]);
    }

    #[test]
    fn focus_cycles_with_three_panes() {
        let mut layout = Layout::new();
        layout.split(Orientation::Vertical);
        layout.split(Orientation::Vertical);
        let ids = layout.leaf_ids();
        assert_eq!(ids.len(), 3);
        layout.active = ids[0];
        layout.focus_next();
        assert_eq!(layout.active, ids[1]);
        layout.focus_next();
        assert_eq!(layout.active, ids[2]);
        layout.focus_next(); // wraps
        assert_eq!(layout.active, ids[0]);
    }

    // ── focus history ─────────────────────────────────────────────────────────

    #[test]
    fn close_returns_to_previously_active_pane() {
        let mut layout = Layout::new();
        let _first = layout.active;           // pane 0
        let second = layout.split(Orientation::Vertical); // pane 1, active
        let third = layout.split(Orientation::Horizontal); // pane 2, active
        // History: [0, 1], active: 2
        // Focus to second, then close → should return to third (prev in history)
        layout.focus_prev(); // go to 1 (history records 2)
        assert_eq!(layout.active, second);
        layout.close_active(); // close 1 → should go back to 2
        assert_eq!(layout.active, third);
    }

    #[test]
    fn close_skips_stale_history_entries() {
        let mut layout = Layout::new();
        let first = layout.active;           // pane 0
        let second = layout.split(Orientation::Vertical); // pane 1
        let third = layout.split(Orientation::Horizontal); // pane 2
        // History: [0, 1], active: 2
        // Close pane 2 → should go to pane 1
        layout.close_active();
        assert_eq!(layout.active, second);
        // Close pane 1 → should go to pane 0 (pane 2 is stale in history)
        layout.close_active();
        assert_eq!(layout.active, first);
    }

    #[test]
    fn split_then_close_returns_to_parent() {
        let mut layout = Layout::new();
        let original = layout.active;
        let _new = layout.split(Orientation::Vertical);
        // Close the new pane → should return to original
        layout.close_active();
        assert_eq!(layout.active, original);
    }
}
