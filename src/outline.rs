//! Resolved outlines: [`Command`], [`Path`], [`Outline`], and [`Bbox`].
//!
//! An [`Outline`] is the *resolved* form a renderer or writer consumes: a
//! vector of closed [`Path`]s, every segment absolute, in font units. Nothing
//! here is curve-type specific — a quadratic and a cubic segment sit in the
//! same enum, so a `glyf` (quadratic) outline and a CFF (cubic) outline both
//! land in the same shape.
//!
//! # Exactness
//!
//! Two things in this module are computed *exactly*, not approximated by
//! flattening:
//!
//! - [`Bbox`] solves each curve's derivative for its axis extrema, so the box
//!   contains the curve and not merely its control hull.
//! - [`Path::signed_area`] integrates `x dy − y dx` in the Bézier's power
//!   basis, so a circle's area comes out as the circle's area.
//!
//! Both properties are what a rasterizer's bounds check and a writer's area
//! heuristic depend on.
//!
//! # Winding
//!
//! [`Path::winding`] names the sign of the signed area. Positive is
//! counter-clockwise in a y-up system. TrueType's outer contours run
//! clockwise in font units, so a typical `'O'` reports [`Winding::Clockwise`].
//!
//! ```
//! use font_model::{Path, Winding};
//!
//! // A unit square, counter-clockwise in y-up.
//! let mut p = Path::starting_at(0.0, 0.0);
//! p.line_to(1.0, 0.0);
//! p.line_to(1.0, 1.0);
//! p.line_to(0.0, 1.0);
//! p.close();
//!
//! assert!((p.signed_area() - 1.0).abs() < 1e-6);
//! assert_eq!(p.winding(), Winding::CounterClockwise);
//!
//! // Relative and absolute construction agree exactly.
//! let mut r = Path::starting_at(0.0, 0.0);
//! r.line_by(1.0, 0.0);
//! r.line_by(0.0, 1.0);
//! r.line_by(-1.0, 0.0);
//! r.close();
//! assert_eq!(r.commands(), p.commands());
//! ```

use alloc::vec;
use alloc::vec::Vec;

/// One resolved path segment, absolute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// Start a new subpath at this point.
    MoveTo(f32, f32),
    /// Straight segment to this point.
    LineTo(f32, f32),
    /// Quadratic segment: control point, then the on-curve endpoint.
    QuadTo(f32, f32, f32, f32),
    /// Cubic segment: two control points, then the on-curve endpoint.
    CubicTo(f32, f32, f32, f32, f32, f32),
    /// Close the current subpath. The line back to the subpath start is
    /// *implicit*: the stored command list gains no synthetic `LineTo`, but
    /// [`Path::area`], [`Path::bbox`], and every consumer of a
    /// [`Path`] account for it.
    Close,
}

impl Command {
    /// The on-curve point this command ends at (`None` for
    /// [`Command::Close`]).
    #[must_use]
    pub fn end_point(&self) -> Option<(f32, f32)> {
        match *self {
            Command::MoveTo(x, y) | Command::LineTo(x, y) => Some((x, y)),
            Command::QuadTo(_, _, x, y) | Command::CubicTo(_, _, _, _, x, y) => Some((x, y)),
            Command::Close => None,
        }
    }

    /// [`Command::MoveTo`] as a standalone command.
    #[must_use]
    pub const fn move_to(x: f32, y: f32) -> Self {
        Command::MoveTo(x, y)
    }

    /// [`Command::LineTo`] as a standalone command.
    #[must_use]
    pub const fn line_to(x: f32, y: f32) -> Self {
        Command::LineTo(x, y)
    }

    /// [`Command::QuadTo`] as a standalone command.
    #[must_use]
    pub const fn quad_to(cx: f32, cy: f32, x: f32, y: f32) -> Self {
        Command::QuadTo(cx, cy, x, y)
    }

    /// [`Command::CubicTo`] as a standalone command.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn cubic_to(c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> Self {
        Command::CubicTo(c1x, c1y, c2x, c2y, x, y)
    }

    /// [`Command::Close`] as a standalone command.
    #[must_use]
    pub const fn close() -> Self {
        Command::Close
    }

    /// Rewrite this command as deltas from `current`, the point the segment
    /// starts at: `(opcode, [dx, dy, dx2, dy2])`.
    ///
    /// The model stores absolute commands; this is the relative view a writer
    /// emits for delta-encoded contours. The array holds
    /// `[dx, dy, dx2, dy2, dx3, dy3]` — control points and endpoint, each as a
    /// delta from `current`; unused trailing slots are zero. Opcodes: 0 move,
    /// 1 line, 2 quad, 3 cubic, 4 close.
    #[must_use]
    pub fn relative_to(&self, current: (f32, f32)) -> (u8, [f32; 6]) {
        let (cx, cy) = current;
        match *self {
            Command::MoveTo(x, y) => (0, [x - cx, y - cy, 0.0, 0.0, 0.0, 0.0]),
            Command::LineTo(x, y) => (1, [x - cx, y - cy, 0.0, 0.0, 0.0, 0.0]),
            Command::QuadTo(c1x, c1y, x, y) => (2, [c1x - cx, c1y - cy, x - cx, y - cy, 0.0, 0.0]),
            Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                (3, [c1x - cx, c1y - cy, c2x - cx, c2y - cy, x - cx, y - cy])
            }
            Command::Close => (4, [0.0; 6]),
        }
    }
}

/// An axis-aligned bounding box in font units.
///
/// The box is empty until a point is added; an empty box contains nothing and
/// has zero extent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bbox {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Default for Bbox {
    fn default() -> Self {
        Self::empty()
    }
}

impl Bbox {
    /// The empty box.
    #[must_use]
    pub const fn empty() -> Self {
        Bbox {
            min_x: f32::INFINITY,
            min_y: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            max_y: f32::NEG_INFINITY,
        }
    }

    /// A box spanning two corners, in any order.
    #[must_use]
    pub fn from_corners(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Bbox {
            min_x: if x0 < x1 { x0 } else { x1 },
            min_y: if y0 < y1 { y0 } else { y1 },
            max_x: if x0 > x1 { x0 } else { x1 },
            max_y: if y0 > y1 { y0 } else { y1 },
        }
    }

    /// The box of an arbitrary point set.
    #[must_use]
    pub fn from_points(points: impl IntoIterator<Item = (f32, f32)>) -> Self {
        let mut b = Bbox::empty();
        for (x, y) in points {
            b.extend(x, y);
        }
        b
    }

    /// Grow the box to include a point.
    pub fn extend(&mut self, x: f32, y: f32) {
        if x < self.min_x {
            self.min_x = x;
        }
        if y < self.min_y {
            self.min_y = y;
        }
        if x > self.max_x {
            self.max_x = x;
        }
        if y > self.max_y {
            self.max_y = y;
        }
    }

    /// Grow the box to include `other` (a no-op for an empty `other`).
    pub fn union(&mut self, other: &Bbox) {
        if other.is_empty() {
            return;
        }
        self.extend(other.min_x, other.min_y);
        self.extend(other.max_x, other.max_y);
    }

    /// True when no point has been added.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.min_x > self.max_x
    }

    /// Minimum x (`f32::INFINITY` when empty).
    #[must_use]
    pub const fn min_x(&self) -> f32 {
        self.min_x
    }

    /// Minimum y (`f32::INFINITY` when empty).
    #[must_use]
    pub const fn min_y(&self) -> f32 {
        self.min_y
    }

    /// Maximum x (`f32::NEG_INFINITY` when empty).
    #[must_use]
    pub const fn max_x(&self) -> f32 {
        self.max_x
    }

    /// Maximum y (`f32::NEG_INFINITY` when empty).
    #[must_use]
    pub const fn max_y(&self) -> f32 {
        self.max_y
    }

    /// Width; zero when empty.
    #[must_use]
    pub fn width(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.max_x - self.min_x
        }
    }

    /// Height; zero when empty.
    #[must_use]
    pub fn height(&self) -> f32 {
        if self.is_empty() {
            0.0
        } else {
            self.max_y - self.min_y
        }
    }

    /// True when the point is inside or on the boundary. An empty box
    /// contains nothing.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        !self.is_empty() && x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Translate the box (an empty box stays empty).
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Bbox {
        if self.is_empty() {
            return Bbox::empty();
        }
        Bbox::from_corners(
            self.min_x + dx,
            self.min_y + dy,
            self.max_x + dx,
            self.max_y + dy,
        )
    }

    /// The four corners, counter-clockwise from the minimum corner.
    #[must_use]
    pub fn corners(&self) -> [(f32, f32); 4] {
        [
            (self.min_x, self.min_y),
            (self.max_x, self.min_y),
            (self.max_x, self.max_y),
            (self.min_x, self.max_y),
        ]
    }
}

/// Orientation of a closed path, by the sign of its signed area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winding {
    /// Positive signed area: counter-clockwise in a y-up system.
    CounterClockwise,
    /// Negative signed area: clockwise in a y-up system (TrueType's outer
    /// contour direction).
    Clockwise,
    /// Zero signed area: a degenerate contour — a single point, a zero-area
    /// sliver, or an open path with no drawn segments.
    Degenerate,
}

/// One subpath: an ordered run of [`Command`]s with a cached current point.
///
/// Every `*_to` constructor is absolute; every `*_by` constructor is relative
/// to the current point and resolves to the *identical* absolute command, so
/// a path built either way compares equal.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    commands: Vec<Command>,
    start: (f32, f32),
    current: (f32, f32),
}

impl Path {
    /// An empty path.
    #[must_use]
    pub fn new() -> Self {
        Path {
            commands: Vec::new(),
            start: (0.0, 0.0),
            current: (0.0, 0.0),
        }
    }

    /// A path holding a single `MoveTo` — the usual first call.
    #[must_use]
    pub fn starting_at(x: f32, y: f32) -> Self {
        let mut p = Path::new();
        p.move_to(x, y);
        p
    }

    /// The commands, in order.
    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Number of commands.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// True when the path holds no commands.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// The current point: the end of the last on-curve command, the subpath
    /// start after a `Close`, or `(0, 0)` for an empty path.
    #[must_use]
    pub const fn current_point(&self) -> (f32, f32) {
        self.current
    }

    /// The start of the current subpath.
    #[must_use]
    pub const fn start_point(&self) -> (f32, f32) {
        self.start
    }

    /// Absolute `MoveTo`, starting a new subpath.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.start = (x, y);
        self.current = (x, y);
        self.commands.push(Command::MoveTo(x, y));
        self
    }

    /// Relative `MoveTo`.
    pub fn move_by(&mut self, dx: f32, dy: f32) -> &mut Self {
        let (x, y) = self.current;
        self.move_to(x + dx, y + dy)
    }

    /// Absolute `LineTo`.
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.current = (x, y);
        self.commands.push(Command::LineTo(x, y));
        self
    }

    /// Relative `LineTo`.
    pub fn line_by(&mut self, dx: f32, dy: f32) -> &mut Self {
        let (x, y) = self.current;
        self.line_to(x + dx, y + dy)
    }

    /// Absolute `QuadTo`.
    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.current = (x, y);
        self.commands.push(Command::QuadTo(cx, cy, x, y));
        self
    }

    /// Relative `QuadTo`: control point and endpoint both relative to the
    /// current point.
    pub fn quad_by(&mut self, dcx: f32, dcy: f32, dx: f32, dy: f32) -> &mut Self {
        let (x, y) = self.current;
        self.quad_to(x + dcx, y + dcy, x + dx, y + dy)
    }

    /// Absolute `CubicTo`.
    pub fn cubic_to(
        &mut self,
        c1x: f32,
        c1y: f32,
        c2x: f32,
        c2y: f32,
        x: f32,
        y: f32,
    ) -> &mut Self {
        self.current = (x, y);
        self.commands
            .push(Command::CubicTo(c1x, c1y, c2x, c2y, x, y));
        self
    }

    /// Relative `CubicTo`: both control points and the endpoint relative to
    /// the current point.
    #[allow(clippy::too_many_arguments)]
    pub fn cubic_by(
        &mut self,
        dc1x: f32,
        dc1y: f32,
        dc2x: f32,
        dc2y: f32,
        dx: f32,
        dy: f32,
    ) -> &mut Self {
        let (x, y) = self.current;
        self.cubic_to(x + dc1x, y + dc1y, x + dc2x, y + dc2y, x + dx, y + dy)
    }

    /// `Close`: append the explicit `Close` command and reset the current
    /// point to the subpath start.
    pub fn close(&mut self) -> &mut Self {
        self.commands.push(Command::Close);
        self.current = self.start;
        self
    }

    /// Drop the trailing `Close`, leaving the subpath open. `None` when the
    /// path was not closed.
    pub fn open(&mut self) -> Option<Command> {
        if !self.is_closed() {
            return None;
        }
        let popped = self.commands.pop();
        self.recache();
        popped
    }

    /// Truncate to `len` commands. `None` when `len` exceeds the length.
    ///
    /// The cached current point and subpath start are recomputed, so the path
    /// stays internally consistent.
    pub fn truncate(&mut self, len: usize) -> Option<()> {
        if len > self.commands.len() {
            return None;
        }
        self.commands.truncate(len);
        self.recache();
        Some(())
    }

    /// Append a raw command, keeping the cached current point in step.
    pub fn push(&mut self, cmd: Command) -> &mut Self {
        match cmd {
            Command::MoveTo(x, y) => {
                self.move_to(x, y);
            }
            Command::LineTo(x, y) => {
                self.line_to(x, y);
            }
            Command::QuadTo(cx, cy, x, y) => {
                self.quad_to(cx, cy, x, y);
            }
            Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                self.cubic_to(c1x, c1y, c2x, c2y, x, y);
            }
            Command::Close => {
                self.close();
            }
        }
        self
    }

    /// True when the path ends with an explicit `Close`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        matches!(self.commands.last(), Some(Command::Close))
    }

    /// Close the path unless it is empty or already closed. Idempotent.
    pub fn ensure_closed(&mut self) -> &mut Self {
        if !self.commands.is_empty() && !self.is_closed() {
            self.close();
        }
        self
    }

    /// Exact bounding box: line endpoints plus the true interior extrema of
    /// every quadratic and cubic. `None` for a path with no points.
    ///
    /// ```
    /// use font_model::Path;
    ///
    /// // A cubic from (0,0) through controls (3,0) (3,3) to (0,3): the exact
    /// // box reaches well inside the control hull's x = 3.
    /// let mut p = Path::starting_at(0.0, 0.0);
    /// p.cubic_to(3.0, 0.0, 3.0, 3.0, 0.0, 3.0);
    /// p.close();
    /// let b = p.bbox().expect("non-empty");
    /// assert!(b.max_x() < 3.0);
    ///
    /// // The known extreme of this cubic: x peaks at 2.25.
    /// assert!((b.max_x() - 2.25).abs() < 1e-5, "max_x {}", b.max_x());
    /// ```
    #[must_use]
    pub fn bbox(&self) -> Option<Bbox> {
        let mut b = Bbox::empty();
        let mut cur = (0.0, 0.0);
        let mut start = (0.0, 0.0);
        let mut any = false;
        for cmd in &self.commands {
            match *cmd {
                Command::MoveTo(x, y) => {
                    b.extend(x, y);
                    cur = (x, y);
                    start = cur;
                    any = true;
                }
                Command::LineTo(x, y) => {
                    b.extend(x, y);
                    cur = (x, y);
                    any = true;
                }
                Command::QuadTo(cx, cy, x, y) => {
                    let p = [(cur.0, cur.1), (cx, cy), (x, y)];
                    b.extend(x, y);
                    // Each axis has its own interior extremum, at its own `t`.
                    for t in quad_derivative_roots(p[0].0, p[1].0, p[2].0) {
                        b.extend(quad_at_x(p, t), quad_at_y(p, t));
                    }
                    for t in quad_derivative_roots(p[0].1, p[1].1, p[2].1) {
                        b.extend(quad_at_x(p, t), quad_at_y(p, t));
                    }
                    cur = (x, y);
                    any = true;
                }
                Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    let p = [(cur.0, cur.1), (c1x, c1y), (c2x, c2y), (x, y)];
                    b.extend(x, y);
                    // Each axis has its own interior extrema, at its own `t`.
                    for t in cubic_derivative_roots(p[0].0, p[1].0, p[2].0, p[3].0) {
                        b.extend(cubic_at_x(p, t), cubic_at_y(p, t));
                    }
                    for t in cubic_derivative_roots(p[0].1, p[1].1, p[2].1, p[3].1) {
                        b.extend(cubic_at_x(p, t), cubic_at_y(p, t));
                    }
                    cur = (x, y);
                    any = true;
                }
                Command::Close => {
                    cur = start;
                    any = true;
                }
            }
        }
        if any {
            Some(b)
        } else {
            None
        }
    }

    /// Signed area of the path in square font units, treating every subpath
    /// as closed. Curves contribute their exact analytic integral, so this is
    /// not a flattened approximation.
    #[must_use]
    pub fn signed_area(&self) -> f32 {
        let mut acc = 0.0f32;
        for seg in self.subpaths() {
            acc += seg.signed_area();
        }
        acc
    }

    /// Unsigned area.
    #[must_use]
    pub fn area(&self) -> f32 {
        crate::num::abs(self.signed_area())
    }

    /// Orientation by signed-area sign.
    #[must_use]
    pub fn winding(&self) -> Winding {
        let a = self.signed_area();
        if a > 0.0 {
            Winding::CounterClockwise
        } else if a < 0.0 {
            Winding::Clockwise
        } else {
            Winding::Degenerate
        }
    }

    /// Reverse the direction of every subpath, keeping the same geometry.
    /// The way to normalise a mixed-direction contour set.
    pub fn reverse(&mut self) {
        let subpaths = self.subpaths();
        let mut out: Vec<Command> = Vec::with_capacity(self.commands.len());
        for seg in &subpaths {
            if seg.points.len() < 2 {
                continue;
            }
            if seg.segments.is_empty() {
                continue;
            }
            // Original segment `i` runs `points[i] → points[i + 1]`, so its
            // reversal runs `points[i + 1] → points[i]` and the reversed
            // subpath begins at the last drawn point.
            let start = *seg.points.first().unwrap_or(&(0.0, 0.0));
            let begin = seg.segments.last().map_or(start, |s| s.to());
            out.push(Command::MoveTo(begin.0, begin.1));
            for (i, s) in seg.segments.iter().enumerate().rev() {
                let to = match i.checked_sub(1).and_then(|k| seg.segments.get(k)) {
                    Some(prev) => prev.to(),
                    None => start,
                };
                match *s {
                    Segment::Line { .. } => out.push(Command::LineTo(to.0, to.1)),
                    Segment::Quad { c, .. } => out.push(Command::QuadTo(c.0, c.1, to.0, to.1)),
                    Segment::Cubic { c1, c2, .. } => {
                        out.push(Command::CubicTo(c2.0, c2.1, c1.0, c1.1, to.0, to.1));
                    }
                }
            }
            if seg.closed {
                out.push(Command::Close);
            }
        }
        self.commands = out;
        self.recache();
    }

    /// Every on-curve and control point, in command order.
    pub fn points(&self) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for cmd in &self.commands {
            match *cmd {
                Command::MoveTo(x, y) | Command::LineTo(x, y) => out.push((x, y)),
                Command::QuadTo(cx, cy, x, y) => {
                    out.push((cx, cy));
                    out.push((x, y));
                }
                Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    out.push((c1x, c1y));
                    out.push((c2x, c2y));
                    out.push((x, y));
                }
                Command::Close => {}
            }
        }
        out
    }

    /// The path split into its subpaths, each with its on-curve points and
    /// segments in order. Each subpath's point list repeats its start at the
    /// end when closed.
    #[must_use]
    pub fn subpaths(&self) -> Vec<Subpath> {
        let mut out: Vec<Subpath> = Vec::new();
        let mut cur: Option<Subpath> = None;
        let mut pt = (0.0f32, 0.0f32);
        for cmd in &self.commands {
            match *cmd {
                Command::MoveTo(x, y) => {
                    if let Some(sp) = cur.take() {
                        out.push(sp);
                    }
                    pt = (x, y);
                    cur = Some(Subpath {
                        points: vec![pt],
                        segments: Vec::new(),
                        closed: false,
                    });
                }
                Command::LineTo(x, y) => {
                    if let Some(sp) = cur.as_mut() {
                        sp.segments.push(Segment::Line { to: (x, y) });
                        sp.points.push((x, y));
                    }
                    pt = (x, y);
                }
                Command::QuadTo(cx, cy, x, y) => {
                    if let Some(sp) = cur.as_mut() {
                        sp.segments.push(Segment::Quad {
                            c: (cx, cy),
                            to: (x, y),
                        });
                        sp.points.push((x, y));
                    }
                    pt = (x, y);
                }
                Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                    if let Some(sp) = cur.as_mut() {
                        sp.segments.push(Segment::Cubic {
                            c1: (c1x, c1y),
                            c2: (c2x, c2y),
                            to: (x, y),
                        });
                        sp.points.push((x, y));
                    }
                    pt = (x, y);
                }
                Command::Close => {
                    if let Some(mut sp) = cur.take() {
                        sp.closed = true;
                        let start = sp.points.first().copied().unwrap_or(pt);
                        sp.points.push(start);
                        out.push(sp);
                    }
                }
            }
        }
        // An unterminated subpath keeps its points as drawn: no repeated
        // start. Its implicit closing line is accounted for by
        // [`Subpath::signed_area`], not by a duplicate point.
        if let Some(sp) = cur.take() {
            out.push(sp);
        }
        out
    }

    /// Recompute the cached current point and subpath start from the
    /// command list. Called after a bulk rewrite of `commands`.
    fn recache(&mut self) {
        self.current = (0.0, 0.0);
        self.start = (0.0, 0.0);
        for cmd in &self.commands {
            match *cmd {
                Command::MoveTo(x, y) => {
                    self.start = (x, y);
                    self.current = (x, y);
                }
                cmd => {
                    if let Some(p) = cmd.end_point() {
                        self.current = p;
                    } else if matches!(cmd, Command::Close) {
                        self.current = self.start;
                    }
                }
            }
        }
    }
}

/// One drawing segment of a decomposed [`Subpath`].
///
/// `to` is the segment's **endpoint**; its start point is the previous
/// segment's endpoint, or the subpath's first point.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Segment {
    Line {
        /// Endpoint.
        to: (f32, f32),
    },
    Quad {
        /// Control point.
        c: (f32, f32),
        /// Endpoint.
        to: (f32, f32),
    },
    Cubic {
        /// First control point.
        c1: (f32, f32),
        /// Second control point.
        c2: (f32, f32),
        /// Endpoint.
        to: (f32, f32),
    },
}

impl Segment {
    /// The segment's endpoint.
    fn to(&self) -> (f32, f32) {
        match *self {
            Segment::Line { to } | Segment::Quad { to, .. } | Segment::Cubic { to, .. } => to,
        }
    }
}

/// One drawing segment, flattened for a consumer that wants the numbers
/// rather than the curve type.
///
/// `kind` is 1 line, 2 quad, 3 cubic. Unused control slots hold `(0.0, 0.0)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlatSegment {
    /// 1 line, 2 quad, 3 cubic.
    pub kind: u8,
    /// First control point, `(0.0, 0.0)` for a line.
    pub c1: (f32, f32),
    /// Second control point, `(0.0, 0.0)` unless this is a cubic.
    pub c2: (f32, f32),
    /// The segment's endpoint.
    pub to: (f32, f32),
}

/// One subpath decomposed into on-curve points and segments.
#[derive(Debug, Clone, PartialEq)]
pub struct Subpath {
    /// On-curve points: the start, each segment endpoint, and (when closed)
    /// a repeat of the start.
    pub points: Vec<(f32, f32)>,
    /// The drawing segments, in order.
    segments: Vec<Segment>,
    /// True when the subpath ended with an explicit `Close`.
    pub closed: bool,
}

impl Subpath {
    /// Number of drawing segments.
    #[must_use]
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// The drawing segments, in order, as `(kind, c1, c2, to)` with `kind`:
    /// 1 line, 2 quad, 3 cubic. Control slots are `(0.0, 0.0)` when unused.
    ///
    /// ```
    /// use font_model::{Path, Winding};
    ///
    /// // A square: four line segments, no controls.
    /// let mut p = Path::starting_at(0.0, 0.0);
    /// p.line_to(1.0, 0.0);
    /// p.line_to(1.0, 1.0);
    /// p.line_to(0.0, 1.0);
    /// p.close();
    /// let subs = p.subpaths();
    /// assert_eq!(subs.len(), 1);
    /// assert_eq!(subs[0].segment_count(), 3);
    /// ```
    #[must_use]
    pub fn segments(&self) -> Vec<FlatSegment> {
        self.segments
            .iter()
            .map(|s| match *s {
                Segment::Line { to } => FlatSegment {
                    kind: 1,
                    c1: (0.0, 0.0),
                    c2: (0.0, 0.0),
                    to,
                },
                Segment::Quad { c, to } => FlatSegment {
                    kind: 2,
                    c1: c,
                    c2: (0.0, 0.0),
                    to,
                },
                Segment::Cubic { c1, c2, to } => FlatSegment {
                    kind: 3,
                    c1,
                    c2,
                    to,
                },
            })
            .collect()
    }

    /// Signed area: each segment's exact analytic contribution plus the
    /// closing line when the subpath is open but has drawn segments.
    #[must_use]
    pub fn signed_area(&self) -> f32 {
        let Some(start) = self.points.first() else {
            return 0.0;
        };
        let mut from = *start;
        let mut acc = 0.0f32;
        for seg in &self.segments {
            acc += segment_area(from, seg);
            from = seg.to();
        }
        // Implicit closing line back to the start.
        if !self.points.is_empty() {
            acc += cross(from, *start);
        }
        acc * 0.5
    }

    /// Exact bounding box over the subpath, `None` when it has no points.
    #[must_use]
    pub fn bbox(&self) -> Option<Bbox> {
        let mut b = Bbox::empty();
        for p in &self.points {
            b.extend(p.0, p.1);
        }
        let mut from = self.points.first().copied().unwrap_or((0.0, 0.0));
        for seg in &self.segments {
            match *seg {
                Segment::Line { .. } => {}
                Segment::Quad { c, to } => {
                    let p = [from, c, to];
                    for t in quad_derivative_roots(p[0].0, p[1].0, p[2].0) {
                        b.extend(quad_at_x(p, t), quad_at_y(p, t));
                    }
                    for t in quad_derivative_roots(p[0].1, p[1].1, p[2].1) {
                        b.extend(quad_at_x(p, t), quad_at_y(p, t));
                    }
                }
                Segment::Cubic { c1, c2, to } => {
                    let p = [from, c1, c2, to];
                    for t in cubic_derivative_roots(p[0].0, p[1].0, p[2].0, p[3].0) {
                        b.extend(cubic_at_x(p, t), cubic_at_y(p, t));
                    }
                    for t in cubic_derivative_roots(p[0].1, p[1].1, p[2].1, p[3].1) {
                        b.extend(cubic_at_x(p, t), cubic_at_y(p, t));
                    }
                }
            }
            from = seg.to();
        }
        if b.is_empty() {
            None
        } else {
            Some(b)
        }
    }
}

/// A glyph's resolved outline: contours in font units.
///
/// The model stores contours *closed* — [`Outline::push_contour`] closes what
/// it is handed, and [`Outline::validate`] is what proves it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    contours: Vec<Path>,
}

impl Outline {
    /// An empty outline (a space-like glyph).
    #[must_use]
    pub fn new() -> Self {
        Outline {
            contours: Vec::new(),
        }
    }

    /// The contours.
    #[must_use]
    pub fn contours(&self) -> &[Path] {
        &self.contours
    }

    /// The contours, mutably — for an editor reshaping a glyph.
    pub fn contours_mut(&mut self) -> &mut Vec<Path> {
        &mut self.contours
    }

    /// Add a contour, closing it first if needed.
    pub fn push_contour(&mut self, path: Path) -> &mut Self {
        let mut path = path;
        path.ensure_closed();
        self.contours.push(path);
        self
    }

    /// Number of contours.
    #[must_use]
    pub fn len(&self) -> usize {
        self.contours.len()
    }

    /// True when the outline has no contours.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Number of on-curve and control points across every contour.
    #[must_use]
    pub fn point_count(&self) -> usize {
        self.contours.iter().map(Path::len).sum()
    }

    /// Exact bounding box over every contour, `None` when there are none.
    #[must_use]
    pub fn bbox(&self) -> Option<Bbox> {
        let mut b = Bbox::empty();
        for c in &self.contours {
            for sp in c.subpaths() {
                if let Some(sb) = sp.bbox() {
                    b.union(&sb);
                }
            }
        }
        if b.is_empty() {
            None
        } else {
            Some(b)
        }
    }

    /// Total unsigned area: outer contours and holes alike.
    #[must_use]
    pub fn area(&self) -> f32 {
        self.contours.iter().map(Path::area).sum()
    }

    /// Net signed area: outer contours minus holes when the holes run the
    /// other way.
    #[must_use]
    pub fn signed_area(&self) -> f32 {
        self.contours.iter().map(Path::signed_area).sum()
    }

    /// Validate the outline against the model invariants: every contour
    /// closed, every coordinate finite.
    ///
    /// # Errors
    /// [`ModelError::UnclosedContour`] naming the contour index, or
    /// [`ModelError::NonFiniteCoordinate`] naming the contour, command index,
    /// and field (`"x"`, `"control1.y"`, …).
    ///
    /// [`ModelError::UnclosedContour`]: crate::ModelError::UnclosedContour
    /// [`ModelError::NonFiniteCoordinate`]: crate::ModelError::NonFiniteCoordinate
    pub fn validate(&self, glyph: crate::GlyphId) -> Result<(), crate::ModelError> {
        use crate::ModelError;
        for (ci, contour) in self.contours.iter().enumerate() {
            if !contour.is_closed() {
                return Err(ModelError::UnclosedContour { glyph, contour: ci });
            }
            for (ki, cmd) in contour.commands().iter().enumerate() {
                let check = |x: f32, y: f32, xf: &'static str, yf: &'static str| {
                    if !x.is_finite() {
                        return Err(ModelError::NonFiniteCoordinate {
                            glyph,
                            contour: ci,
                            command: ki,
                            field: xf,
                        });
                    }
                    if !y.is_finite() {
                        return Err(ModelError::NonFiniteCoordinate {
                            glyph,
                            contour: ci,
                            command: ki,
                            field: yf,
                        });
                    }
                    Ok(())
                };
                match *cmd {
                    Command::MoveTo(x, y) | Command::LineTo(x, y) => check(x, y, "x", "y")?,
                    Command::QuadTo(cx, cy, x, y) => {
                        check(cx, cy, "control.x", "control.y")?;
                        check(x, y, "x", "y")?;
                    }
                    Command::CubicTo(c1x, c1y, c2x, c2y, x, y) => {
                        check(c1x, c1y, "control1.x", "control1.y")?;
                        check(c2x, c2y, "control2.x", "control2.y")?;
                        check(x, y, "x", "y")?;
                    }
                    Command::Close => {}
                }
            }
        }
        Ok(())
    }
}

// -- exact curve helpers ---------------------------------------------------

/// `cross(a, b)` = `a.x * b.y − a.y * b.x`.
fn cross(a: (f32, f32), b: (f32, f32)) -> f32 {
    a.0 * b.1 - a.1 * b.0
}

/// Exact `∫₀¹ (x y′ − y x′) dt` contribution of one segment, via the
/// Bézier's power basis. Straight lines use the closed form `cross(p₀, p₁)`.
fn segment_area(from: (f32, f32), seg: &Segment) -> f32 {
    match *seg {
        Segment::Line { to } => cross(from, to),
        Segment::Quad { c, to } => {
            let xp = quadratic_power(from.0, c.0, to.0);
            let yp = quadratic_power(from.1, c.1, to.1);
            integrate_cross(&xp, &yp)
        }
        Segment::Cubic { c1, c2, to } => {
            let xp = cubic_power(from.0, c1.0, c2.0, to.0);
            let yp = cubic_power(from.1, c1.1, c2.1, to.1);
            integrate_cross(&xp, &yp)
        }
    }
}

/// Power-basis coefficients of a quadratic Bézier, ascending in `t`.
fn quadratic_power(p0: f32, p1: f32, p2: f32) -> [f32; 3] {
    [p0, 2.0 * (p1 - p0), (p0 - 2.0 * p1 + p2)]
}

/// Power-basis coefficients of a cubic Bézier, ascending in `t`.
fn cubic_power(p0: f32, p1: f32, p2: f32, p3: f32) -> [f32; 4] {
    [
        p0,
        3.0 * (p1 - p0),
        3.0 * (p0 - 2.0 * p1 + p2),
        (p3 - 3.0 * p2 + 3.0 * p1 - p0),
    ]
}

/// Differentiate a power-basis polynomial (length shrinks by one).
fn derivative(p: &[f32]) -> Vec<f32> {
    p.iter()
        .enumerate()
        .skip(1)
        .map(|(i, c)| c * i as f32)
        .collect()
}

/// Multiply two power-basis polynomials.
fn mul(a: &[f32], b: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; a.len() + b.len() - 1];
    for (i, &ai) in a.iter().enumerate() {
        for (j, &bj) in b.iter().enumerate() {
            if let Some(slot) = out.get_mut(i + j) {
                *slot += ai * bj;
            }
        }
    }
    out
}

/// `∫₀¹ (x y′ − y x′) dt` for two power-basis polynomials.
fn integrate_cross(xp: &[f32], yp: &[f32]) -> f32 {
    let a = mul(xp, &derivative(yp));
    let b = mul(yp, &derivative(xp));
    let mut acc = 0.0f32;
    for (i, ca) in a.iter().enumerate() {
        let cb = b.get(i).copied().unwrap_or(0.0);
        let k = (i + 1) as f32;
        acc += (ca - cb) / k;
    }
    acc
}

/// Quadratic x at `t`.
fn quad_at_x(p: [(f32, f32); 3], t: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * p[0].0 + 2.0 * mt * t * p[1].0 + t * t * p[2].0
}

/// Quadratic y at `t`.
fn quad_at_y(p: [(f32, f32); 3], t: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * p[0].1 + 2.0 * mt * t * p[1].1 + t * t * p[2].1
}

/// Interior roots of a quadratic Bézier's derivative on one axis — where the
/// curve itself reaches an axis extreme.
fn quad_derivative_roots(a0: f32, a1: f32, a2: f32) -> Vec<f32> {
    let qa = 2.0 * (a0 - 2.0 * a1 + a2);
    let qb = 2.0 * (a1 - a0);
    if crate::num::abs(qa) < 1e-9 {
        return Vec::new();
    }
    let t = -qb / qa;
    if t > 0.0 && t < 1.0 {
        vec![t]
    } else {
        Vec::new()
    }
}

/// Cubic x at `t`.
fn cubic_at_x(p: [(f32, f32); 4], t: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * mt * p[0].0
        + 3.0 * mt * mt * t * p[1].0
        + 3.0 * mt * t * t * p[2].0
        + t * t * t * p[3].0
}

/// Cubic y at `t`.
fn cubic_at_y(p: [(f32, f32); 4], t: f32) -> f32 {
    let mt = 1.0 - t;
    mt * mt * mt * p[0].1
        + 3.0 * mt * mt * t * p[1].1
        + 3.0 * mt * t * t * p[2].1
        + t * t * t * p[3].1
}

/// Interior roots of a cubic Bézier's derivative on one axis — the curve's
/// own axis extrema, exact rather than a control-hull bound.
fn cubic_derivative_roots(a0: f32, a1: f32, a2: f32, a3: f32) -> Vec<f32> {
    let qa = 3.0 * (-a0 + 3.0 * a1 - 3.0 * a2 + a3);
    let qb = 6.0 * (a0 - 2.0 * a1 + a2);
    let qc = 3.0 * (a1 - a0);
    let mut out = Vec::new();
    let mut push = |t: f32| {
        if t > 0.0 && t < 1.0 {
            out.push(t);
        }
    };
    if crate::num::abs(qa) < 1e-9 {
        if crate::num::abs(qb) >= 1e-9 {
            push(-qc / qb);
        }
        return out;
    }
    let disc = qb * qb - 4.0 * qa * qc;
    if disc >= 0.0 {
        let s = crate::num::sqrt(disc);
        push((-qb + s) / (2.0 * qa));
        push((-qb - s) / (2.0 * qa));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::float_cmp
    )]
    use super::{Bbox, Command, Outline, Path, Subpath, Winding};
    use alloc::vec;

    // Test harness: assertions legitimately panic; the lib target stays
    // lint-clean.

    /// A 10×10 square starting at the origin, counter-clockwise.
    fn square() -> Path {
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(10.0, 0.0);
        p.line_to(10.0, 10.0);
        p.line_to(0.0, 10.0);
        p.close();
        p
    }

    #[test]
    fn bbox_of_lines() {
        let b = square().bbox().expect("bbox");
        assert_eq!((b.min_x(), b.min_y()), (0.0, 0.0));
        assert_eq!((b.max_x(), b.max_y()), (10.0, 10.0));
        assert_eq!(b.width(), 10.0);
        assert_eq!(b.height(), 10.0);
    }

    #[test]
    fn bbox_cubic_holds_the_curve_not_the_hull() {
        // (0,0) → (3,0) → (3,3) → (0,3): x peaks at 2.25 inside the hull's 3.
        let mut p = Path::starting_at(0.0, 0.0);
        p.cubic_to(3.0, 0.0, 3.0, 3.0, 0.0, 3.0);
        p.close();
        let b = p.bbox().expect("bbox");
        assert!(b.max_x() < 3.0, "bbox {} includes the hull", b.max_x());
        assert!((b.max_x() - 2.25).abs() < 1e-4, "max_x {}", b.max_x());
        assert!((b.max_y() - 3.0).abs() < 1e-5);
    }

    #[test]
    fn bbox_quadratic_interior_extremum() {
        // (0,0) → (2,4) → (4,0): max y = 2 at t = 0.5.
        let mut p = Path::starting_at(0.0, 0.0);
        p.quad_to(2.0, 4.0, 4.0, 0.0);
        p.close();
        let b = p.bbox().expect("bbox");
        assert!((b.max_y() - 2.0).abs() < 1e-5, "max_y {}", b.max_y());
        assert!((b.max_x() - 4.0).abs() < 1e-5, "max_x {}", b.max_x());
    }

    #[test]
    fn bbox_contains_every_sampled_point() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.cubic_to(3.0, -2.0, -4.0, 5.0, 1.0, 2.0);
        p.quad_to(2.0, 2.0, 0.0, 0.0);
        p.close();
        let b = p.bbox().expect("bbox");
        let a = [(0.0, 0.0), (3.0, -2.0), (-4.0, 5.0), (1.0, 2.0)];
        for i in 0..=200 {
            let t = i as f32 / 200.0;
            let mt = 1.0 - t;
            let x = mt * mt * mt * a[0].0
                + 3.0 * mt * mt * t * a[1].0
                + 3.0 * mt * t * t * a[2].0
                + t * t * t * a[3].0;
            let y = mt * mt * mt * a[0].1
                + 3.0 * mt * mt * t * a[1].1
                + 3.0 * mt * t * t * a[2].1
                + t * t * t * a[3].1;
            assert!(b.contains(x, y), "sample ({x}, {y}) outside {b:?}");
        }
    }

    #[test]
    fn bbox_of_empty_and_point_only_paths() {
        assert!(Path::new().bbox().is_none());
        let mut p = Path::starting_at(1.0, 1.0);
        p.close();
        let b = p.bbox().expect("point bbox");
        assert_eq!(b.width(), 0.0);
    }

    #[test]
    fn area_and_winding_sign() {
        let ccw = square();
        assert!((ccw.signed_area() - 100.0).abs() < 1e-3);
        assert_eq!(ccw.winding(), Winding::CounterClockwise);
        assert_eq!(ccw.area(), 100.0);

        let mut cw = Path::starting_at(0.0, 0.0);
        cw.line_to(0.0, 10.0);
        cw.line_to(10.0, 10.0);
        cw.line_to(10.0, 0.0);
        cw.close();
        assert!((cw.signed_area() + 100.0).abs() < 1e-3);
        assert_eq!(cw.winding(), Winding::Clockwise);
        assert_eq!(cw.area(), 100.0);
    }

    #[test]
    fn degenerate_winding() {
        let mut p = Path::starting_at(1.0, 1.0);
        p.close();
        assert_eq!(p.winding(), Winding::Degenerate);
        assert_eq!(p.area(), 0.0);
        let open = Path::starting_at(0.0, 0.0);
        assert_eq!(open.winding(), Winding::Degenerate);
    }

    /// Four quadratics approximating the unit circle. A quadratic's control
    /// point is the *tangent intersection* (1, 1) — the 0.552 magic constant is
    /// a cubic one — so the shape is a coarse approximation and the area
    /// tolerance is correspondingly loose.
    fn quad_circle() -> Path {
        let mut c = Path::starting_at(1.0, 0.0);
        c.quad_to(1.0, 1.0, 0.0, 1.0);
        c.quad_to(-1.0, 1.0, -1.0, 0.0);
        c.quad_to(-1.0, -1.0, 0.0, -1.0);
        c.quad_to(1.0, -1.0, 1.0, 0.0);
        c.close();
        c
    }

    /// Four cubic arcs approximating the unit circle, with the standard
    /// `4/3·tan(π/8)` control offset 0.55228475.
    fn cubic_circle() -> Path {
        let k = 0.552_284_8_f32;
        let mut c = Path::starting_at(1.0, 0.0);
        c.cubic_to(1.0, k, k, 1.0, 0.0, 1.0);
        c.cubic_to(-k, 1.0, -1.0, k, -1.0, 0.0);
        c.cubic_to(-1.0, -k, -k, -1.0, 0.0, -1.0);
        c.cubic_to(k, -1.0, 1.0, -k, 1.0, 0.0);
        c.close();
        c
    }

    #[test]
    fn quadratic_circle_area_matches_its_own_polygon() {
        // Green's theorem, checked against the exact analytic value: for four
        // quadratics with control points at the tangent intersections, the
        // enclosed area is exactly 10/3 (the polygonal approximation of a
        // circle inscribed in the unit square's rotated twin), well above pi.
        let c = quad_circle();
        assert!(
            (c.signed_area() - 3.333_333_3).abs() < 1e-3,
            "quad circle area {}",
            c.signed_area()
        );
        assert_eq!(c.winding(), Winding::CounterClockwise);
        // And it is still close to the circle it approximates.
        assert!(
            (c.area() - core::f32::consts::PI).abs() < 0.2,
            "quad circle area {}",
            c.area()
        );
    }

    #[test]
    fn cubic_circle_area_is_pi_to_1e_3() {
        // Four cubic arcs approximate the circle to ~3e-4 in area, so the
        // tolerance is set by the *approximation*, not the area integral — the
        // integral itself is exact for these curves.
        let c = cubic_circle();
        assert!(
            (c.signed_area() - core::f32::consts::PI).abs() < 1e-3,
            "cubic circle area {}",
            c.signed_area()
        );
        let b = c.bbox().expect("bbox");
        assert!((b.max_x() - 1.0).abs() < 1e-6 && (b.min_y() + 1.0).abs() < 1e-6);
    }

    #[test]
    fn absolute_and_relative_agree() {
        let mut a = Path::starting_at(0.0, 0.0);
        a.line_to(10.0, 0.0);
        a.quad_to(12.0, 4.0, 10.0, 8.0);
        a.cubic_to(8.0, 10.0, 4.0, 10.0, 0.0, 8.0);
        a.close();

        let mut b = Path::starting_at(0.0, 0.0);
        b.line_by(10.0, 0.0);
        b.quad_by(2.0, 4.0, 0.0, 8.0);
        b.cubic_by(-2.0, 2.0, -6.0, 2.0, -10.0, 0.0);
        b.close();

        assert_eq!(a.commands(), b.commands());
        assert_eq!(a.signed_area(), b.signed_area());
        assert_eq!(a.bbox(), b.bbox());
    }

    #[test]
    fn move_by_starts_a_subpath() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(1.0, 0.0);
        // Relative to the current point (1, 0), so the new subpath starts at
        // (2, 1) — the delta is not applied to the origin.
        p.move_by(1.0, 1.0);
        p.line_to(2.0, 1.0);
        p.close();
        assert_eq!(p.start_point(), (2.0, 1.0));
        assert_eq!(p.len(), 5);
        assert_eq!(p.subpaths().len(), 2);
    }

    #[test]
    fn close_is_implicit_but_accounted() {
        let mut open = Path::starting_at(0.0, 0.0);
        open.line_to(10.0, 0.0);
        open.line_to(10.0, 10.0);
        open.line_to(0.0, 10.0);
        assert!(!open.is_closed());
        let closed = {
            let mut c = open.clone();
            c.close();
            c
        };
        assert_eq!(open.len() + 1, closed.len());
        // The area is the same: the closing line is implicit either way.
        assert!((open.signed_area() - closed.signed_area()).abs() < 1e-4);
        assert!((closed.area() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn ensure_closed_is_idempotent() {
        let mut p = square();
        p.ensure_closed();
        assert_eq!(p.len(), 5);
        let mut e = Path::new();
        e.ensure_closed();
        assert!(e.is_empty());
    }

    #[test]
    fn end_point_of_each_command() {
        assert_eq!(Command::move_to(1.0, 2.0).end_point(), Some((1.0, 2.0)));
        assert_eq!(Command::line_to(1.0, 2.0).end_point(), Some((1.0, 2.0)));
        assert_eq!(
            Command::quad_to(0.0, 0.0, 1.0, 2.0).end_point(),
            Some((1.0, 2.0))
        );
        assert_eq!(
            Command::cubic_to(0.0, 0.0, 0.0, 0.0, 1.0, 2.0).end_point(),
            Some((1.0, 2.0))
        );
        assert_eq!(Command::close().end_point(), None);
    }

    #[test]
    fn relative_to_reports_every_opcode() {
        let cur = (3.0, 4.0);
        assert_eq!(
            Command::line_to(5.0, 6.0).relative_to(cur),
            (1, [2.0, 2.0, 0.0, 0.0, 0.0, 0.0])
        );
        assert_eq!(Command::move_to(3.0, 4.0).relative_to(cur), (0, [0.0; 6]));
        assert_eq!(
            Command::quad_to(3.0, 4.0, 5.0, 6.0).relative_to(cur),
            (2, [0.0, 0.0, 2.0, 2.0, 0.0, 0.0])
        );
        let (op, d) = Command::cubic_to(3.0, 4.0, 5.0, 6.0, 7.0, 8.0).relative_to(cur);
        assert_eq!(op, 3);
        assert_eq!(d, [0.0, 0.0, 2.0, 2.0, 4.0, 4.0]);
        assert_eq!(Command::close().relative_to(cur), (4, [0.0; 6]));
    }

    #[test]
    fn outline_aggregates_and_nets_holes() {
        let mut o = Outline::new();
        assert!(o.is_empty());
        assert!(o.bbox().is_none());
        assert_eq!(o.area(), 0.0);
        assert_eq!(o.point_count(), 0);
        o.push_contour(square());
        let mut hole = Path::starting_at(2.0, 2.0);
        hole.line_to(2.0, 8.0);
        hole.line_to(8.0, 8.0);
        hole.line_to(8.0, 2.0);
        o.push_contour(hole);
        assert_eq!(o.len(), 2);
        // The hole runs clockwise, so the net area is the 6x6... ring.
        assert!((o.signed_area() - 64.0).abs() < 1e-3, "{}", o.signed_area());
        // Total unsigned area counts both.
        assert!((o.area() - 136.0).abs() < 1e-3, "{}", o.area());
        let b = o.bbox().expect("bbox");
        assert_eq!(b.max_x(), 10.0);
        assert_eq!(o.point_count(), 10);
    }

    #[test]
    fn outline_push_closes_and_validate_accepts() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(1.0, 0.0);
        p.line_to(1.0, 1.0);
        assert!(!p.is_closed());
        let mut o = Outline::new();
        o.push_contour(p);
        assert!(o.contours()[0].is_closed());
        assert!(o.validate(crate::GlyphId::new(0)).is_ok());
    }

    #[test]
    fn validate_reports_unclosed_contour_index() {
        let mut o = Outline::new();
        o.push_contour(square());
        let mut q = Path::starting_at(0.0, 0.0);
        q.line_to(1.0, 1.0);
        o.contours_mut().push(q);
        match o.validate(crate::GlyphId::new(5)).unwrap_err() {
            crate::ModelError::UnclosedContour { glyph, contour } => {
                assert_eq!(glyph.to_u32(), 5);
                assert_eq!(contour, 1);
            }
            other => panic!("expected UnclosedContour, got {other:?}"),
        }
    }

    #[test]
    fn validate_reports_each_non_finite_field() {
        let cases: [(f32, f32, f32, &str); 1] = [(f32::NAN, 0.0, 0.0, "x")];
        for (x, _y, _z, field) in cases {
            let mut o = Outline::new();
            let mut p = Path::starting_at(x, 0.0);
            p.line_to(1.0, 1.0);
            p.close();
            o.push_contour(p);
            let e = o.validate(crate::GlyphId::new(1)).unwrap_err();
            match e {
                crate::ModelError::NonFiniteCoordinate {
                    glyph,
                    contour,
                    command,
                    field: got,
                } => {
                    assert_eq!(glyph.to_u32(), 1);
                    assert_eq!(contour, 0);
                    assert_eq!(command, 0);
                    assert_eq!(got, field);
                }
                other => panic!("expected NonFiniteCoordinate, got {other:?}"),
            }
        }

        // Infinite control y on a cubic names control1.y.
        let mut o = Outline::new();
        let mut s = Path::starting_at(0.0, 0.0);
        s.cubic_to(0.0, f32::INFINITY, 1.0, 1.0, 1.0, 1.0);
        s.close();
        o.push_contour(s);
        assert!(matches!(
            o.validate(crate::GlyphId::new(2)).unwrap_err(),
            crate::ModelError::NonFiniteCoordinate {
                field: "control1.y",
                ..
            }
        ));

        // Infinite quad endpoint x names x.
        let mut o = Outline::new();
        let mut q = Path::starting_at(0.0, 0.0);
        q.quad_to(1.0, 1.0, f32::NEG_INFINITY, 1.0);
        q.close();
        o.push_contour(q);
        assert!(matches!(
            o.validate(crate::GlyphId::new(3)).unwrap_err(),
            crate::ModelError::NonFiniteCoordinate { field: "x", .. }
        ));

        // An infinite quad *control* names control.x, so the control is checked
        // before the endpoint.
        let mut o = Outline::new();
        let mut q = Path::starting_at(0.0, 0.0);
        q.quad_to(f32::INFINITY, 1.0, f32::NEG_INFINITY, 1.0);
        q.close();
        o.push_contour(q);
        assert!(matches!(
            o.validate(crate::GlyphId::new(7)).unwrap_err(),
            crate::ModelError::NonFiniteCoordinate {
                field: "control.x",
                ..
            }
        ));

        // Infinite control2.x on a cubic names control2.x.
        let mut o = Outline::new();
        let mut r = Path::starting_at(0.0, 0.0);
        r.cubic_to(1.0, 1.0, f32::INFINITY, 1.0, 1.0, 1.0);
        r.close();
        o.push_contour(r);
        assert!(matches!(
            o.validate(crate::GlyphId::new(4)).unwrap_err(),
            crate::ModelError::NonFiniteCoordinate {
                field: "control2.x",
                ..
            }
        ));

        // Infinite control.x on a quad names control.x.
        let mut o = Outline::new();
        let mut t = Path::starting_at(0.0, 0.0);
        t.quad_to(f32::INFINITY, 1.0, 1.0, 1.0);
        t.close();
        o.push_contour(t);
        assert!(matches!(
            o.validate(crate::GlyphId::new(6)).unwrap_err(),
            crate::ModelError::NonFiniteCoordinate {
                field: "control.x",
                ..
            }
        ));
    }

    #[test]
    fn reverse_flips_winding() {
        let mut p = square();
        p.reverse();
        assert_eq!(p.winding(), Winding::Clockwise);
        assert_eq!(p.area(), 100.0);
        assert_eq!(p.len(), 5);
        assert!(p.is_closed());
    }

    #[test]
    fn reverse_preserves_curve_geometry() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.cubic_to(3.0, 0.0, 3.0, 3.0, 0.0, 3.0);
        p.quad_to(-3.0, 3.0, 0.0, 0.0);
        p.close();
        let before_area = p.area();
        let before_bbox = p.bbox().expect("bbox");
        p.reverse();
        // Reversing flips the direction, so the winding flips with it — the
        // geometry is unchanged.
        assert_eq!(p.winding(), Winding::Clockwise);
        assert!((p.area() - before_area).abs() < 1e-3);
        let after = p.bbox().expect("bbox");
        assert!((after.min_x() - before_bbox.min_x()).abs() < 1e-4);
        assert!((after.max_x() - before_bbox.max_x()).abs() < 1e-4);
        assert!((after.min_y() - before_bbox.min_y()).abs() < 1e-4);
        assert!((after.max_y() - before_bbox.max_y()).abs() < 1e-4);
        // Reversing twice restores the geometry and the winding. The command
        // *list* need not come back: a quad followed by a cubic reverses to a
        // cubic followed by a quad, which is an equivalent but different
        // stream — so the invariant checked here is geometric, not textual.
        p.reverse();
        assert_eq!(p.winding(), Winding::CounterClockwise);
        let back = p.bbox().expect("bbox");
        assert!((back.min_x() - before_bbox.min_x()).abs() < 1e-4);
        assert!((back.max_y() - before_bbox.max_y()).abs() < 1e-4);
        assert!((p.area() - before_area).abs() < 1e-3);
    }

    #[test]
    fn reverse_multi_subpath_keeps_areas() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(1.0, 0.0);
        p.line_to(1.0, 1.0);
        p.move_to(5.0, 5.0);
        p.line_to(6.0, 5.0);
        p.line_to(6.0, 6.0);
        p.close();
        let before = p.area();
        p.reverse();
        assert!((p.area() - before).abs() < 1e-4);
        assert!(p.is_closed());
        assert_eq!(p.subpaths().len(), 2);
    }

    #[test]
    fn reverse_of_a_path_with_no_segments_is_empty() {
        // A lone `MoveTo` + `Close` draws nothing, so there is nothing to
        // reverse: the result is empty rather than a `MoveTo` with no shape.
        let mut p = Path::starting_at(0.0, 0.0);
        p.close();
        p.reverse();
        assert!(p.is_empty());
        assert_eq!(p.area(), 0.0);
        let mut e = Path::new();
        e.reverse();
        assert!(e.is_empty());
    }

    #[test]
    fn points_lists_controls() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.quad_to(1.0, 1.0, 2.0, 0.0);
        p.cubic_to(3.0, 0.0, 4.0, 0.0, 5.0, 0.0);
        p.close();
        let pts = p.points();
        // MoveTo + quad(control, end) + cubic(2 controls, end); `Close` adds
        // none.
        assert_eq!(pts.len(), 6);
        assert_eq!(pts[0], (0.0, 0.0));
        assert_eq!(pts[1], (1.0, 1.0));
    }

    #[test]
    fn push_matches_typed_constructors() {
        let mut a = Path::starting_at(1.0, 2.0);
        a.quad_to(3.0, 4.0, 5.0, 6.0);
        let mut b = Path::new();
        b.push(Command::MoveTo(1.0, 2.0));
        b.push(Command::QuadTo(3.0, 4.0, 5.0, 6.0));
        assert_eq!(a.commands(), b.commands());
        assert_eq!(b.current_point(), (5.0, 6.0));
        assert_eq!(b.start_point(), (1.0, 2.0));
    }

    #[test]
    fn subpath_exposes_segments() {
        let mut p = Path::starting_at(0.0, 0.0);
        p.line_to(1.0, 0.0);
        p.quad_to(1.0, 1.0, 0.0, 1.0);
        p.cubic_to(-1.0, 1.0, -1.0, 0.0, 0.0, 0.0);
        p.close();
        let subs = p.subpaths();
        assert_eq!(subs.len(), 1);
        let sp: &Subpath = &subs[0];
        assert!(sp.closed);
        assert_eq!(sp.segment_count(), 3);
        assert_eq!(sp.points.len(), 5, "start + 3 ends + closing repeat");
        let segs = sp.segments();
        assert_eq!(segs[0].kind, 1);
        assert_eq!(segs[1].kind, 2);
        assert_eq!(segs[2].kind, 3);
        assert_eq!(segs[0].c1, (0.0, 0.0), "a line has no controls");
        assert_eq!(segs[1].c1, (1.0, 1.0), "the quad's control point");
        // The subpath's area is the path's area — the two views agree exactly,
        // which is the property that makes `subpaths` usable as a decomposition.
        assert_eq!(sp.signed_area(), p.signed_area());
        assert!(sp.signed_area() > 0.0, "counter-clockwise");
        // The cubic `(0,1) → (-1,1) (-1,0) → (0,0)` reaches x = -0.75 at its
        // midpoint; the exact box finds it, a control hull would say -1.
        assert!((sp.bbox().expect("bbox").min_x() + 0.75).abs() < 1e-5);
    }

    #[test]
    fn subpaths_of_open_and_empty_paths() {
        assert!(Path::new().subpaths().is_empty());
        let open = Path::starting_at(0.0, 0.0);
        let subs = open.subpaths();
        assert_eq!(subs.len(), 1);
        assert!(!subs[0].closed);
        assert_eq!(subs[0].signed_area(), 0.0);
        assert!(subs[0].bbox().is_some());
        // An open subpath does not repeat its start point; a closed one does.
        let mut closed = Path::starting_at(0.0, 0.0);
        closed.line_to(1.0, 0.0);
        closed.close();
        assert_eq!(closed.subpaths()[0].points.len(), 3);
        let mut unclosed = Path::starting_at(0.0, 0.0);
        unclosed.line_to(1.0, 0.0);
        assert_eq!(unclosed.subpaths()[0].points.len(), 2);
        // An open path's area still accounts for its implicit closing line.
        let mut tri = Path::starting_at(0.0, 0.0);
        tri.line_to(10.0, 0.0);
        tri.line_to(0.0, 10.0);
        assert!((tri.signed_area() - 50.0).abs() < 1e-4);
    }

    #[test]
    fn bbox_helpers() {
        assert!(Bbox::empty().is_empty());
        assert_eq!(Bbox::empty().width(), 0.0);
        assert_eq!(Bbox::empty().height(), 0.0);
        assert!(!Bbox::empty().contains(0.0, 0.0));
        let b = Bbox::from_corners(10.0, 20.0, 0.0, 5.0);
        assert_eq!(b.min_x(), 0.0);
        assert_eq!(b.max_y(), 20.0);
        assert!(b.contains(5.0, 10.0));
        assert!(b.contains(0.0, 5.0), "boundary counts as inside");
        assert!(!b.contains(-1.0, 10.0));
        let t = b.translated(1.0, 1.0);
        assert_eq!(t.min_x(), 1.0);
        assert!(Bbox::empty().translated(1.0, 1.0).is_empty());
        let mut u = Bbox::from_points(vec![(0.0, 0.0), (3.0, 4.0)]);
        assert_eq!(u.max_y(), 4.0);
        let snapshot = u;
        u.union(&Bbox::from_corners(-1.0, -1.0, -1.0, -1.0));
        assert_eq!(u.min_x(), -1.0);
        assert_eq!(snapshot.min_x(), 0.0, "union took by value");
        u.union(&Bbox::empty());
        assert_eq!(u.min_x(), -1.0);
        assert_eq!(Bbox::default(), Bbox::empty());
        assert_eq!(b.corners()[0], (0.0, 5.0));
        assert_eq!(b.corners()[3], (0.0, 20.0));
    }

    #[test]
    fn many_points_bbox_contains_all() {
        let mut p = Path::starting_at(0.0, 0.0);
        for i in 1..100 {
            p.line_to(f32::from(u16::try_from(i).unwrap_or(99)), 0.0);
        }
        let b = p.bbox().expect("bbox");
        assert_eq!(b.max_x(), 99.0);
        for pt in p.points() {
            assert!(b.contains(pt.0, pt.1), "point {pt:?} outside {b:?}");
        }
    }

    #[test]
    fn outline_default_is_empty() {
        let o = Outline::default();
        assert!(o.is_empty());
        assert_eq!(o.contours().len(), 0);
        assert_eq!(o.signed_area(), 0.0);
    }
}
