//! The annotation document: what has been drawn, in what order, and how to
//! undo it.
//!
//! The scene never touches pixels. It holds the *instructions* to reproduce an
//! annotated image, which is what lets the editor and the exporter render the
//! same result at different resolutions, and lets any step be undone.

use crate::drawable::Drawable;
use crate::math::{Rect, Vec2D};
use crate::painter::Painter;

/// One undoable step.
#[derive(Debug)]
pub enum Operation {
    /// An annotation was committed.
    Draw(Box<dyn Drawable>),
    /// The image was cropped to `rect`, which was expressed in the coordinate
    /// space *before* the crop. Undoing restores `previous_size` and shifts
    /// every annotation back.
    Crop { rect: Rect, previous_size: Vec2D },
    /// An existing annotation was dragged. `target` indexes into `operations`.
    ///
    /// The index stays valid because operations are only ever pushed and
    /// popped at the end: undoing far enough to remove the annotation would
    /// have to undo this move first.
    Move { target: usize, delta: Vec2D },
    /// An existing annotation was deleted.
    ///
    /// The annotation itself is *not* carried here: the original `Draw` stays
    /// in the log, and deletion works by this record shadowing it, so undo
    /// only has to pop. Cloning the drawable as well would cost a full copy
    /// per delete and be read by nothing.
    Delete { target: usize },
}

impl Operation {
    pub fn kind(&self) -> &'static str {
        match self {
            Operation::Draw(d) => d.kind(),
            Operation::Crop { .. } => "crop",
            Operation::Move { .. } => "move",
            Operation::Delete { .. } => "delete",
        }
    }
}

/// The annotation document.
#[derive(Debug, Default)]
pub struct Scene {
    size: Vec2D,
    /// Applied operations, oldest first.
    operations: Vec<Operation>,
    /// Undone operations, most recently undone last.
    redo_stack: Vec<Operation>,
}

impl Scene {
    pub fn new(size: Vec2D) -> Self {
        Self {
            size,
            operations: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Current image size in pixels.
    pub fn size(&self) -> Vec2D {
        self.size
    }

    /// The full canvas rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::new(Vec2D::ZERO, self.size)
    }

    /// Replace the backing image, discarding all history. Used when a new
    /// capture is loaded into an existing editor.
    pub fn reset(&mut self, size: Vec2D) {
        self.size = size;
        self.operations.clear();
        self.redo_stack.clear();
    }

    /// Committed annotations, oldest first, skipping any that were deleted.
    pub fn drawables(&self) -> impl Iterator<Item = &dyn Drawable> {
        self.operations
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.is_deleted(*i))
            .filter_map(|(_, op)| match op {
                Operation::Draw(d) => Some(d.as_ref()),
                _ => None,
            })
    }

    /// Whether the annotation at `index` has since been deleted.
    fn is_deleted(&self, index: usize) -> bool {
        self.operations
            .iter()
            .any(|op| matches!(op, Operation::Delete { target, .. } if *target == index))
    }

    /// The annotation at an index returned by [`Scene::hit_test`].
    pub fn annotation(&self, index: usize) -> Option<&dyn Drawable> {
        match self.operations.get(index)? {
            Operation::Draw(d) if !self.is_deleted(index) => Some(d.as_ref()),
            _ => None,
        }
    }

    /// Move a committed annotation, undoably.
    ///
    /// Deltas are translation-invariant, so a later crop (which translates
    /// everything) does not invalidate a recorded move.
    pub fn move_annotation(&mut self, index: usize, delta: Vec2D) -> bool {
        if delta.is_zero() || !self.nudge_annotation(index, delta) {
            return false;
        }
        self.record_move(index, delta);
        true
    }

    /// Move an annotation *without* recording history.
    ///
    /// Dragging emits a move every frame; recording each one would bury the
    /// undo stack under hundreds of one-pixel steps. So a drag nudges live and
    /// calls [`Scene::record_move`] once, with the total, when it ends.
    pub fn nudge_annotation(&mut self, index: usize, delta: Vec2D) -> bool {
        if self.annotation(index).is_none() {
            return false;
        }
        match self.operations.get_mut(index) {
            Some(Operation::Draw(d)) => {
                d.translate(delta);
                true
            }
            _ => false,
        }
    }

    /// Record an already-applied move so it can be undone. Pairs with
    /// [`Scene::nudge_annotation`]; do not call it for a move that has not
    /// been applied yet.
    pub fn record_move(&mut self, index: usize, total_delta: Vec2D) -> bool {
        if total_delta.is_zero() || index >= self.operations.len() {
            return false;
        }
        self.operations.push(Operation::Move {
            target: index,
            delta: total_delta,
        });
        self.redo_stack.clear();
        true
    }

    /// Delete a committed annotation, undoably.
    pub fn delete_annotation(&mut self, index: usize) -> bool {
        if self.annotation(index).is_none() {
            return false;
        }
        self.operations.push(Operation::Delete { target: index });
        self.redo_stack.clear();
        true
    }

    pub fn annotation_count(&self) -> usize {
        self.drawables().count()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Commit an annotation. This invalidates the redo stack, as in every
    /// other editor: you cannot redo into a future that no longer follows.
    pub fn add(&mut self, drawable: Box<dyn Drawable>) {
        self.operations.push(Operation::Draw(drawable));
        self.redo_stack.clear();
    }

    /// Crop the image. Every annotation is rebased onto the new origin, so
    /// annotation coordinates stay relative to the visible image.
    ///
    /// Returns false when the crop is empty or would not change anything.
    pub fn apply_crop(&mut self, rect: Rect) -> bool {
        let rect = rect.normalized().clamped_to(self.bounds()).rounded();
        if rect.is_empty() || (rect.pos.is_zero() && rect.size == self.size) {
            return false;
        }

        for op in &mut self.operations {
            if let Operation::Draw(d) = op {
                d.translate(-rect.pos);
            }
        }
        let previous_size = self.size;
        self.size = rect.size;
        self.operations.push(Operation::Crop {
            rect,
            previous_size,
        });
        self.redo_stack.clear();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.operations.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(op) = self.operations.pop() else {
            return false;
        };
        match &op {
            Operation::Crop {
                rect,
                previous_size,
            } => {
                self.size = *previous_size;
                let delta = rect.pos;
                for other in &mut self.operations {
                    if let Operation::Draw(d) = other {
                        d.translate(delta);
                    }
                }
            }
            Operation::Move { target, delta } => {
                let (target, delta) = (*target, -*delta);
                if let Some(Operation::Draw(d)) = self.operations.get_mut(target) {
                    d.translate(delta);
                }
            }
            // Delete carries the annotation with it; popping the operation is
            // enough to make it visible again.
            Operation::Delete { .. } | Operation::Draw(_) => {}
        }
        self.redo_stack.push(op);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(op) = self.redo_stack.pop() else {
            return false;
        };
        match &op {
            Operation::Crop { rect, .. } => {
                let delta = -rect.pos;
                self.size = rect.size;
                for other in &mut self.operations {
                    if let Operation::Draw(d) = other {
                        d.translate(delta);
                    }
                }
            }
            Operation::Move { target, delta } => {
                let (target, delta) = (*target, *delta);
                if let Some(Operation::Draw(d)) = self.operations.get_mut(target) {
                    d.translate(delta);
                }
            }
            Operation::Delete { .. } | Operation::Draw(_) => {}
        }
        self.operations.push(op);
        true
    }

    /// Drop every annotation but keep the image. Undoable as a single step is
    /// *not* supported: this is the explicit "reset" action.
    pub fn clear(&mut self) {
        self.operations.clear();
        self.redo_stack.clear();
    }

    /// Paint every committed annotation, oldest first.
    pub fn draw(&self, painter: &mut dyn Painter) {
        for drawable in self.drawables() {
            drawable.draw(painter);
        }
    }

    /// The number a newly placed marker should carry.
    ///
    /// One past the highest number currently on screen, not the count of
    /// markers. Counting looks equivalent and is not: delete marker 1 of
    /// [1, 2, 3] and the count says the next is 3, which already exists, so
    /// two markers end up captioned "3". Deriving it from the live drawables
    /// rather than a running counter is still what keeps undo and redo
    /// consistent.
    pub fn next_marker_number(&self) -> u16 {
        self.drawables()
            .filter_map(|d| d.sequence_number())
            .max()
            .map_or(1, |highest| highest.saturating_add(1))
    }

    /// The topmost annotation under `point`, if any. Used by post-paint
    /// selection: later annotations are drawn on top, so they win.
    pub fn hit_test(&self, point: Vec2D) -> Option<usize> {
        self.operations
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, op)| match op {
                Operation::Draw(d) if d.hit_test(point) && !self.is_deleted(i) => Some(i),
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::RecordingPainter;
    use crate::style::Style;
    use crate::tools::{Marker, Rectangle};

    fn rect_at(x: f32, y: f32) -> Box<dyn Drawable> {
        Box::new(Rectangle {
            rect: Rect::from_xywh(x, y, 10.0, 10.0),
            style: Style::default(),
        })
    }

    fn scene() -> Scene {
        Scene::new(Vec2D::new(800.0, 600.0))
    }

    #[test]
    fn annotations_accumulate_and_draw_in_order() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(100.0, 100.0));
        assert_eq!(s.annotation_count(), 2);

        let mut p = RecordingPainter::new();
        s.draw(&mut p);
        assert_eq!(p.strokes().count(), 2);
    }

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(50.0, 50.0));

        assert!(s.undo());
        assert_eq!(s.annotation_count(), 1);
        assert!(s.undo());
        assert_eq!(s.annotation_count(), 0);
        assert!(!s.undo(), "nothing left to undo");

        assert!(s.redo());
        assert!(s.redo());
        assert_eq!(s.annotation_count(), 2);
        assert!(!s.redo(), "nothing left to redo");
    }

    #[test]
    fn drawing_after_an_undo_discards_the_redo_future() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(50.0, 50.0));
        s.undo();
        assert!(s.can_redo());

        s.add(rect_at(200.0, 200.0));
        assert!(!s.can_redo(), "the old future must be dropped");
        assert_eq!(s.annotation_count(), 2);
    }

    #[test]
    fn cropping_resizes_the_canvas_and_rebases_annotations() {
        let mut s = scene();
        s.add(rect_at(100.0, 100.0));

        assert!(s.apply_crop(Rect::from_xywh(50.0, 40.0, 400.0, 300.0)));
        assert_eq!(s.size(), Vec2D::new(400.0, 300.0));

        let bounds = s.drawables().next().unwrap().bounds().unwrap();
        // The annotation was at (100,100); after cropping at (50,40) it is at
        // (50,60) in the new coordinate space.
        assert!((bounds.center().x - 55.0).abs() < 1.0, "{bounds:?}");
        assert!((bounds.center().y - 65.0).abs() < 1.0, "{bounds:?}");
    }

    #[test]
    fn undoing_a_crop_restores_both_the_size_and_the_positions() {
        let mut s = scene();
        s.add(rect_at(100.0, 100.0));
        let before = s.drawables().next().unwrap().bounds().unwrap();

        s.apply_crop(Rect::from_xywh(50.0, 40.0, 400.0, 300.0));
        assert!(s.undo());

        assert_eq!(s.size(), Vec2D::new(800.0, 600.0));
        let after = s.drawables().next().unwrap().bounds().unwrap();
        assert!((after.pos.x - before.pos.x).abs() < 0.01, "{after:?}");
        assert!((after.pos.y - before.pos.y).abs() < 0.01, "{after:?}");
    }

    #[test]
    fn redoing_a_crop_reapplies_it() {
        let mut s = scene();
        s.add(rect_at(100.0, 100.0));
        s.apply_crop(Rect::from_xywh(50.0, 40.0, 400.0, 300.0));
        s.undo();
        assert!(s.redo());
        assert_eq!(s.size(), Vec2D::new(400.0, 300.0));
        let bounds = s.drawables().next().unwrap().bounds().unwrap();
        assert!((bounds.center().x - 55.0).abs() < 1.0, "{bounds:?}");
    }

    #[test]
    fn a_no_op_or_empty_crop_is_rejected() {
        let mut s = scene();
        assert!(!s.apply_crop(s.bounds()), "cropping to the full image");
        assert!(
            !s.apply_crop(Rect::from_xywh(10.0, 10.0, 0.0, 0.0)),
            "empty"
        );
        assert!(s.is_empty(), "neither should enter the history");
    }

    #[test]
    fn a_crop_is_clamped_to_the_image() {
        let mut s = scene();
        assert!(s.apply_crop(Rect::from_xywh(-100.0, -100.0, 400.0, 400.0)));
        // Clipped to the top-left corner of the image.
        assert_eq!(s.size(), Vec2D::new(300.0, 300.0));
    }

    #[test]
    fn crops_compose_and_unwind_in_order() {
        let mut s = scene();
        s.add(rect_at(500.0, 400.0));
        s.apply_crop(Rect::from_xywh(100.0, 100.0, 600.0, 400.0));
        s.apply_crop(Rect::from_xywh(50.0, 50.0, 300.0, 200.0));
        assert_eq!(s.size(), Vec2D::new(300.0, 200.0));

        s.undo();
        assert_eq!(s.size(), Vec2D::new(600.0, 400.0));
        s.undo();
        assert_eq!(s.size(), Vec2D::new(800.0, 600.0));

        let bounds = s.drawables().next().unwrap().bounds().unwrap();
        assert!((bounds.center().x - 505.0).abs() < 0.1, "{bounds:?}");
    }

    #[test]
    fn marker_numbering_follows_the_history() {
        let mut s = scene();
        assert_eq!(s.next_marker_number(), 1);

        for n in 1..=3u16 {
            s.add(Box::new(Marker {
                pos: Vec2D::new(n as f32 * 10.0, 0.0),
                number: n,
                style: Style::default(),
            }));
        }
        assert_eq!(s.next_marker_number(), 4);

        s.undo();
        assert_eq!(s.next_marker_number(), 3, "undo frees the number again");
        s.redo();
        assert_eq!(s.next_marker_number(), 4);
    }

    #[test]
    fn non_markers_do_not_affect_marker_numbering() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(10.0, 10.0));
        assert_eq!(s.next_marker_number(), 1);
    }

    #[test]
    fn hit_testing_prefers_the_annotation_drawn_last() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(0.0, 0.0));
        assert_eq!(s.hit_test(Vec2D::new(5.0, 5.0)), Some(1));
        assert_eq!(s.hit_test(Vec2D::new(700.0, 500.0)), None);
    }

    // --- post-paint editing -------------------------------------------------

    #[test]
    fn a_committed_annotation_can_be_moved_and_the_move_undone() {
        let mut s = scene();
        s.add(rect_at(100.0, 100.0));
        let before = s.drawables().next().unwrap().bounds().unwrap();

        assert!(s.move_annotation(0, Vec2D::new(30.0, -20.0)));
        let after = s.drawables().next().unwrap().bounds().unwrap();
        assert_eq!(after.pos, before.pos + Vec2D::new(30.0, -20.0));
        assert_eq!(after.size, before.size, "moving must not resize");

        assert!(s.undo());
        assert_eq!(
            s.drawables().next().unwrap().bounds().unwrap().pos,
            before.pos
        );
        assert!(s.redo());
        assert_eq!(
            s.drawables().next().unwrap().bounds().unwrap().pos,
            after.pos
        );
    }

    #[test]
    fn a_dragged_move_records_one_undo_step_not_one_per_frame() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        let before = s.drawables().next().unwrap().bounds().unwrap();

        // Simulate a drag: many small nudges, one recorded move.
        let mut total = Vec2D::ZERO;
        for _ in 0..50 {
            let step = Vec2D::new(1.0, 0.5);
            assert!(s.nudge_annotation(0, step));
            total += step;
        }
        assert!(s.record_move(0, total));

        let after = s.drawables().next().unwrap().bounds().unwrap();
        assert_eq!(after.pos, before.pos + Vec2D::new(50.0, 25.0));

        assert_eq!(
            s.operations.len(),
            2,
            "the 50 nudges should have produced exactly one Move alongside the Draw"
        );

        // So a single undo puts it all the way back.
        assert!(s.undo());
        let restored = s.drawables().next().unwrap().bounds().unwrap();
        assert!((restored.pos.x - before.pos.x).abs() < 0.01, "{restored:?}");
        assert_eq!(s.operations.len(), 1, "only the Draw should remain");
    }

    #[test]
    fn moving_nothing_or_by_nothing_is_rejected() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        assert!(!s.move_annotation(0, Vec2D::ZERO), "a zero move is a no-op");
        assert!(
            !s.move_annotation(99, Vec2D::new(1.0, 1.0)),
            "no such index"
        );
        assert_eq!(s.operations.len(), 1, "neither entered the history");
    }

    #[test]
    fn a_deleted_annotation_disappears_and_comes_back_on_undo() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(200.0, 200.0));
        assert_eq!(s.annotation_count(), 2);

        assert!(s.delete_annotation(0));
        assert_eq!(s.annotation_count(), 1);
        assert!(s.annotation(0).is_none());
        assert!(
            s.hit_test(Vec2D::new(5.0, 5.0)).is_none(),
            "and is unclickable"
        );

        assert!(s.undo());
        assert_eq!(s.annotation_count(), 2);
        assert!(s.annotation(0).is_some());
    }

    #[test]
    fn deleting_the_same_annotation_twice_is_rejected() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        assert!(s.delete_annotation(0));
        assert!(!s.delete_annotation(0), "already gone");
        assert!(!s.move_annotation(0, Vec2D::new(5.0, 5.0)), "and unmovable");
    }

    #[test]
    fn a_deleted_annotation_stays_deleted_through_redo() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.delete_annotation(0);
        s.undo();
        assert_eq!(s.annotation_count(), 1);
        s.redo();
        assert_eq!(s.annotation_count(), 0);
    }

    #[test]
    fn moves_survive_a_later_crop_because_deltas_are_translation_invariant() {
        let mut s = scene();
        s.add(rect_at(300.0, 300.0));
        s.move_annotation(0, Vec2D::new(20.0, 10.0));
        s.apply_crop(Rect::from_xywh(100.0, 100.0, 400.0, 400.0));

        let cropped = s.drawables().next().unwrap().bounds().unwrap();
        // 300 + 20 - 100 = 220, and 300 + 10 - 100 = 210.
        assert!((cropped.center().x - 225.0).abs() < 1.0, "{cropped:?}");
        assert!((cropped.center().y - 215.0).abs() < 1.0, "{cropped:?}");

        // Unwinding both steps restores the original position.
        s.undo();
        s.undo();
        let restored = s.drawables().next().unwrap().bounds().unwrap();
        assert!((restored.center().x - 305.0).abs() < 0.1, "{restored:?}");
    }

    #[test]
    fn deleting_a_marker_from_the_middle_never_produces_a_duplicate_number() {
        let mut s = scene();
        for n in 1..=3u16 {
            s.add(Box::new(Marker {
                pos: Vec2D::new(n as f32 * 10.0, 0.0),
                number: n,
                style: Style::default(),
            }));
        }
        assert_eq!(s.next_marker_number(), 4);

        // Remove the first. The remaining markers still read 2 and 3, so the
        // next one must be 4 — counting would have said 3 and collided.
        s.delete_annotation(0);
        assert_eq!(s.next_marker_number(), 4);

        let live: Vec<u16> = s.drawables().filter_map(|d| d.sequence_number()).collect();
        assert_eq!(live, vec![2, 3]);
        assert!(
            !live.contains(&s.next_marker_number()),
            "the next number is already in use"
        );
    }

    #[test]
    fn removing_the_last_marker_frees_its_number_again() {
        let mut s = scene();
        for n in 1..=2u16 {
            s.add(Box::new(Marker {
                pos: Vec2D::new(n as f32 * 10.0, 0.0),
                number: n,
                style: Style::default(),
            }));
        }
        s.undo();
        assert_eq!(s.next_marker_number(), 2, "undo frees the highest number");
        s.redo();
        assert_eq!(s.next_marker_number(), 3);
    }

    #[test]
    fn editing_after_an_undo_discards_the_redo_future() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.add(rect_at(50.0, 50.0));
        s.undo();
        assert!(s.can_redo());
        s.move_annotation(0, Vec2D::new(5.0, 5.0));
        assert!(!s.can_redo());
    }

    #[test]
    fn clear_drops_everything_including_the_redo_stack() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.undo();
        s.clear();
        assert!(s.is_empty());
        assert!(!s.can_undo() && !s.can_redo());
    }

    #[test]
    fn reset_swaps_the_image_and_forgets_the_history() {
        let mut s = scene();
        s.add(rect_at(0.0, 0.0));
        s.reset(Vec2D::new(100.0, 100.0));
        assert_eq!(s.size(), Vec2D::new(100.0, 100.0));
        assert!(s.is_empty());
        assert!(!s.can_undo());
    }
}
