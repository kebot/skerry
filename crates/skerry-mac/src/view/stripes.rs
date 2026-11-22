//! Painting JPEG stripes as separate images, and retiring their atlas tiles (`ARCH-034`).

use gpui_kit::{point, px, size, Bounds, Pixels, RenderImage};
use std::sync::Arc;

/// Paints a replaced stripe stays in the atlas after it leaves the screen. GPUI never waits
/// for the GPU: it allows three drawables in flight (`setMaximumDrawableCount(3)`,
/// metal_renderer.rs:214), and in `DisplayLinkMode::Detached` (the CVDisplayLink fallback,
/// window activation, live resize, occlusion) it takes the drawable *after* the paint
/// (metal_renderer.rs:612). So at paint N+3, frame N may still be sampling a tile last
/// painted in N, and a freed atlas region is reused at once by the next stripe's upload.
const RETIRE_AFTER_PAINTS: u8 = 4;

/// The stripe images on screen, and the replaced ones waiting out `RETIRE_AFTER_PAINTS`
/// before `window.drop_image`.
#[derive(Default)]
pub(super) struct StripeRetirer {
    current: Vec<Arc<RenderImage>>,
    /// Each with the number of paints since it left the screen.
    retired: Vec<(u8, Arc<RenderImage>)>,
}

impl StripeRetirer {
    /// Records one paint showing `current`, and returns the images now due for
    /// `drop_image`: off screen for `RETIRE_AFTER_PAINTS` paints, never one in `current`.
    pub(super) fn painted(&mut self, current: &[Arc<RenderImage>]) -> Vec<Arc<RenderImage>> {
        let shown = |image: &Arc<RenderImage>| current.iter().any(|now| Arc::ptr_eq(now, image));
        for image in std::mem::take(&mut self.current) {
            if !shown(&image) {
                self.retired.push((0, image));
            }
        }
        self.current = current.to_vec();
        self.retired.retain(|(_, image)| !shown(image));
        let mut due = Vec::new();
        self.retired.retain_mut(|(age, image)| {
            *age += 1;
            let keep = *age < RETIRE_AFTER_PAINTS;
            if !keep {
                due.push(image.clone());
            }
            keep
        });
        due
    }

    /// Everything still in the atlas, for a window that is going away.
    pub(super) fn take_all(&mut self) -> Vec<Arc<RenderImage>> {
        let mut all = std::mem::take(&mut self.current);
        all.extend(self.retired.drain(..).map(|(_, image)| image));
        all
    }
}

/// Where one stripe goes, as `paint_image`'s `(bounds, image_bounds)`.
///
/// Every edge is rounded to a whole device pixel first, so GPUI's own snapping (round half
/// toward zero of `top` and `top + height`) cannot split a seam into a gap or an overlap,
/// even when the picture is centred on a half pixel; and stripe i ends exactly where i+1
/// starts. `image_bounds` is the stripe grown by one source row each way, so the repeated
/// padding rows fall outside `bounds` and are cut off.
pub(super) fn stripe_bounds(
    display: Bounds<Pixels>,
    scale_factor: f32,
    remote_height: u16,
    y: u16,
    height: u16,
) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let scale = f64::from(scale_factor.max(f32::EPSILON));
    let device = |logical: Pixels| f64::from(f32::from(logical)) * scale;
    let logical = |device: f64| px((device.round() / scale) as f32);
    let (top, extent) = (device(display.origin.y), device(display.size.height));
    let row = |y: u16| logical(top + f64::from(y) * extent / f64::from(remote_height.max(1)));
    let left = logical(device(display.origin.x));
    let right = logical(device(display.origin.x) + device(display.size.width));
    let (stripe_top, stripe_bottom) = (row(y), row(y.saturating_add(height)));
    let bounds = Bounds::new(
        point(left, stripe_top),
        size(right - left, stripe_bottom - stripe_top),
    );
    let source_row = (stripe_bottom - stripe_top) / f32::from(height.max(1));
    let image_bounds = Bounds::new(
        point(left, stripe_top - source_row),
        size(right - left, bounds.size.height + source_row * 2.),
    );
    (bounds, image_bounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;

    fn image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(smallvec![image::Frame::new(
            image::RgbaImage::new(1, 1)
        )]))
    }

    /// GPUI's `round_to_device_pixel` (util.rs): round half toward zero of `logical * scale`.
    fn snap(logical: Pixels, scale: f32) -> f32 {
        let value = f32::from(logical) * scale;
        (value.abs() - 0.5).ceil().copysign(value)
    }

    fn geometry(remote_height: u16, stripes: u16) -> Vec<(u16, u16)> {
        let base = remote_height / stripes;
        let extra = remote_height % stripes;
        let mut y = 0;
        (0..stripes)
            .map(|i| {
                let height = base + u16::from(i < extra);
                let stripe = (y, height);
                y += height;
                stripe
            })
            .collect()
    }

    #[test]
    fn stripe_edges_meet_after_gpui_snaps_them() {
        let cases = [
            // Match Retina centred on a half device pixel.
            (
                2.0,
                Bounds::new(point(px(10.25), px(7.75)), size(px(746.), px(934.))),
            ),
            (
                1.0,
                Bounds::new(point(px(0.5), px(12.5)), size(px(1492.), px(1868.))),
            ),
            (
                0.77,
                Bounds::new(point(px(3.3), px(1.7)), size(px(1148.84), px(1438.36))),
            ),
            (
                1.5,
                Bounds::new(point(px(0.4), px(9.1)), size(px(994.67), px(1245.33))),
            ),
        ];
        for (scale, display) in cases {
            for count in [4, 8] {
                let stripes = geometry(1868, count);
                let painted: Vec<_> = stripes
                    .iter()
                    .map(|&(y, h)| stripe_bounds(display, scale, 1868, y, h).0)
                    .collect();
                for pair in painted.windows(2) {
                    assert_eq!(
                        snap(pair[0].bottom(), scale),
                        snap(pair[1].top(), scale),
                        "scale {scale}, {count} stripes"
                    );
                }
                let first = painted.first().unwrap();
                let last = painted.last().unwrap();
                assert!((f32::from(first.top() - display.top()) * scale).abs() <= 0.5);
                assert!((f32::from(last.bottom() - display.bottom()) * scale).abs() <= 0.5);
                for stripe in &painted {
                    assert_eq!(snap(stripe.left(), scale), snap(first.left(), scale));
                    assert_eq!(snap(stripe.right(), scale), snap(first.right(), scale));
                }
            }
        }
    }

    #[test]
    fn stripe_image_bounds_pad_one_source_row() {
        let display = Bounds::new(point(px(0.), px(0.)), size(px(746.), px(934.)));
        let (bounds, image_bounds) = stripe_bounds(display, 2.0, 1868, 234, 234);
        let source_row = bounds.size.height / 234.;
        assert_eq!(image_bounds.top(), bounds.top() - source_row);
        assert!((f32::from(image_bounds.bottom() - bounds.bottom() - source_row)).abs() < 1e-3);
        // GPUI's sub-tile for a 236-row tile: offset 1 row, height 234 rows.
        let offset = (bounds.top() - image_bounds.top()) / image_bounds.size.height * 236.;
        let rows = bounds.size.height / image_bounds.size.height * 236.;
        assert_eq!(offset.round(), 1.);
        assert_eq!(rows.round(), 234.);
    }

    #[test]
    fn retirer_drops_only_after_four_paints_and_never_a_current_image() {
        let (a, b, c) = (image(), image(), image());
        let mut retirer = StripeRetirer::default();
        assert!(retirer.painted(&[a.clone(), b.clone()]).is_empty());
        // `b` replaced by `c`: off screen from this paint on.
        for _ in 0..3 {
            assert!(retirer.painted(&[a.clone(), c.clone()]).is_empty());
        }
        let due = retirer.painted(&[a.clone(), c.clone()]);
        assert_eq!(due.len(), 1);
        assert!(Arc::ptr_eq(&due[0], &b));
        // Shown again before it was due: kept, not dropped.
        retirer.painted(std::slice::from_ref(&a));
        retirer.painted(&[a.clone(), c.clone()]);
        for _ in 0..5 {
            assert!(retirer.painted(&[a.clone(), c.clone()]).is_empty());
        }
    }

    #[test]
    fn retirer_take_all_returns_pending_and_current() {
        let (a, b) = (image(), image());
        let mut retirer = StripeRetirer::default();
        retirer.painted(std::slice::from_ref(&a));
        retirer.painted(std::slice::from_ref(&b));
        let all = retirer.take_all();
        assert_eq!(all.len(), 2);
        assert!(all.iter().any(|image| Arc::ptr_eq(image, &a)));
        assert!(all.iter().any(|image| Arc::ptr_eq(image, &b)));
        assert!(retirer.take_all().is_empty());
        assert!(retirer.painted(&[]).is_empty());
    }
}
