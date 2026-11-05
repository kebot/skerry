//! Pure coordinate mapping between the Mirror (logical px) and the remote
//! page (CSS px). The Frame is scaled uniformly to fit the Mirror and
//! centered; leftover space is letterbox bars. All math is `f32`.
//!
//! The remote size is the Frame's `deviceWidth x deviceHeight` (CSS pixels),
//! not the JPEG's bitmap size, so HiDPI bitmaps need no special handling.
//! Degenerate (zero, negative or non-finite) sizes give an empty fit: no
//! point maps and nothing panics.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Point { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    /// Half-open containment: left/top edges inside, right/bottom outside.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }
}

/// How a remote page of a given CSS size is fitted into a Mirror.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameFit {
    remote_w: f32,
    remote_h: f32,
    dest: Rect,
    /// Mirror px per remote CSS px.
    scale: f32,
}

fn valid(v: f32) -> bool {
    v.is_finite() && v > 0.0
}

impl FrameFit {
    pub fn new(remote_w: f32, remote_h: f32, mirror_w: f32, mirror_h: f32) -> FrameFit {
        if !(valid(remote_w) && valid(remote_h) && valid(mirror_w) && valid(mirror_h)) {
            return FrameFit {
                remote_w: 0.0,
                remote_h: 0.0,
                dest: Rect::default(),
                scale: 0.0,
            };
        }
        let scale = (mirror_w / remote_w).min(mirror_h / remote_h);
        let w = remote_w * scale;
        let h = remote_h * scale;
        FrameFit {
            remote_w,
            remote_h,
            dest: Rect {
                x: (mirror_w - w) / 2.0,
                y: (mirror_h - h) / 2.0,
                width: w,
                height: h,
            },
            scale,
        }
    }

    /// From a Frame's metadata (`deviceWidth`/`deviceHeight`).
    pub fn for_frame(meta: &crate::FrameMetadata, mirror_w: f32, mirror_h: f32) -> FrameFit {
        FrameFit::new(
            meta.device_width as f32,
            meta.device_height as f32,
            mirror_w,
            mirror_h,
        )
    }

    /// True if no mapping is possible (a size was zero or invalid).
    pub fn is_empty(&self) -> bool {
        self.scale == 0.0
    }

    /// Where the Frame is drawn inside the Mirror (the rest is letterbox).
    pub fn dest_rect(&self) -> Rect {
        self.dest
    }

    /// Mirror px per remote CSS px (0 if empty).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Map a Mirror point to remote CSS coordinates. Returns `None` for
    /// points in the letterbox bars (outside the Frame) or when empty.
    pub fn mirror_to_remote(&self, p: Point) -> Option<Point> {
        if self.is_empty() || !self.dest.contains(p) {
            return None;
        }
        Some(self.unchecked(p))
    }

    /// Like [`mirror_to_remote`](Self::mirror_to_remote) but clamps points
    /// in the bars to the nearest Frame edge (useful for drags that leave
    /// the page). Empty fits give `(0, 0)`.
    pub fn mirror_to_remote_clamped(&self, p: Point) -> Point {
        if self.is_empty() {
            return Point::default();
        }
        let r = self.unchecked(p);
        Point::new(r.x.clamp(0.0, self.remote_w), r.y.clamp(0.0, self.remote_h))
    }

    /// Map remote CSS coordinates to a Mirror point (inverse of
    /// [`mirror_to_remote`](Self::mirror_to_remote)); not clamped. Empty
    /// fits give `(0, 0)`.
    pub fn remote_to_mirror(&self, p: Point) -> Point {
        if self.is_empty() {
            return Point::default();
        }
        Point::new(
            self.dest.x + p.x * self.scale,
            self.dest.y + p.y * self.scale,
        )
    }

    fn unchecked(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.dest.x) / self.scale,
            (p.y - self.dest.y) / self.scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }
    fn close(a: Point, b: Point) {
        assert!(
            (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn exact_fit_has_no_bars() {
        let f = FrameFit::new(800.0, 600.0, 800.0, 600.0);
        assert_eq!(
            f.dest_rect(),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0
            }
        );
        assert_eq!(f.scale(), 1.0);
        assert_eq!(f.mirror_to_remote(pt(0.0, 0.0)), Some(pt(0.0, 0.0)));
        assert_eq!(f.mirror_to_remote(pt(400.0, 300.0)), Some(pt(400.0, 300.0)));
        // Right/bottom edge is outside (half-open).
        assert_eq!(f.mirror_to_remote(pt(800.0, 300.0)), None);
    }

    #[test]
    fn landscape_page_in_taller_mirror_gets_top_bottom_bars() {
        let f = FrameFit::new(1000.0, 500.0, 500.0, 500.0);
        assert_eq!(f.scale(), 0.5);
        assert_eq!(
            f.dest_rect(),
            Rect {
                x: 0.0,
                y: 125.0,
                width: 500.0,
                height: 250.0
            }
        );
        assert_eq!(f.mirror_to_remote(pt(0.0, 100.0)), None);
        assert_eq!(f.mirror_to_remote(pt(0.0, 400.0)), None);
        close(
            f.mirror_to_remote(pt(250.0, 250.0)).unwrap(),
            pt(500.0, 250.0),
        );
    }

    #[test]
    fn portrait_page_in_wide_mirror_gets_side_bars() {
        let f = FrameFit::new(400.0, 800.0, 1000.0, 400.0);
        assert_eq!(f.scale(), 0.5);
        assert_eq!(
            f.dest_rect(),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 200.0,
                height: 400.0
            }
        );
        assert_eq!(f.mirror_to_remote(pt(399.0, 200.0)), None);
        close(f.mirror_to_remote(pt(400.0, 0.0)).unwrap(), pt(0.0, 0.0));
        close(
            f.mirror_to_remote(pt(500.0, 200.0)).unwrap(),
            pt(200.0, 400.0),
        );
    }

    #[test]
    fn upscales_small_pages() {
        let f = FrameFit::new(100.0, 50.0, 400.0, 400.0);
        assert_eq!(f.scale(), 4.0);
        assert_eq!(f.dest_rect().height, 200.0);
    }

    #[test]
    fn clamped_maps_bars_to_nearest_edge() {
        let f = FrameFit::new(1000.0, 500.0, 500.0, 500.0);
        close(f.mirror_to_remote_clamped(pt(250.0, 0.0)), pt(500.0, 0.0));
        close(
            f.mirror_to_remote_clamped(pt(250.0, 500.0)),
            pt(500.0, 500.0),
        );
        close(f.mirror_to_remote_clamped(pt(-50.0, 250.0)), pt(0.0, 250.0));
        close(
            f.mirror_to_remote_clamped(pt(900.0, 250.0)),
            pt(1000.0, 250.0),
        );
        // Inside: same as the strict mapping.
        close(
            f.mirror_to_remote_clamped(pt(250.0, 250.0)),
            pt(500.0, 250.0),
        );
    }

    #[test]
    fn round_trip() {
        let f = FrameFit::new(1280.0, 720.0, 913.0, 611.0);
        for (x, y) in [
            (0.0, 0.0),
            (1.0, 1.0),
            (640.0, 360.0),
            (1279.0, 719.0),
            (13.7, 500.2),
        ] {
            let m = f.remote_to_mirror(pt(x, y));
            close(f.mirror_to_remote(m).unwrap(), pt(x, y));
        }
    }

    #[test]
    fn hidpi_bitmap_size_is_irrelevant() {
        // A 2x bitmap of a 800x600 CSS page maps the same as a 1x one:
        // only the CSS size (deviceWidth/Height) is used.
        let meta = crate::FrameMetadata {
            device_width: 800.0,
            device_height: 600.0,
            ..Default::default()
        };
        let f = FrameFit::for_frame(&meta, 400.0, 300.0);
        assert_eq!(f.scale(), 0.5);
        close(
            f.mirror_to_remote(pt(200.0, 150.0)).unwrap(),
            pt(400.0, 300.0),
        );
    }

    #[test]
    fn zero_and_invalid_sizes_do_not_panic() {
        for f in [
            FrameFit::new(0.0, 600.0, 800.0, 600.0),
            FrameFit::new(800.0, 0.0, 800.0, 600.0),
            FrameFit::new(800.0, 600.0, 0.0, 600.0),
            FrameFit::new(800.0, 600.0, 800.0, 0.0),
            FrameFit::new(-1.0, 600.0, 800.0, 600.0),
            FrameFit::new(f32::NAN, 600.0, 800.0, 600.0),
            FrameFit::new(f32::INFINITY, 600.0, 800.0, 600.0),
        ] {
            assert!(f.is_empty());
            assert_eq!(f.dest_rect(), Rect::default());
            assert_eq!(f.mirror_to_remote(pt(1.0, 1.0)), None);
            assert_eq!(f.mirror_to_remote_clamped(pt(1.0, 1.0)), pt(0.0, 0.0));
            assert_eq!(f.remote_to_mirror(pt(1.0, 1.0)), pt(0.0, 0.0));
        }
    }
}
