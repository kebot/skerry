use gpui::{App, DisplayId, Pixels, Size, Window};
use objc2_core_foundation::{CFArray, CFRetained};
use objc2_core_graphics::{CGDisplayCopyAllDisplayModes, CGDisplayMode};

// kDisplayModeNativeFlag from IOKit/graphics/IOGraphicsTypes.h.
const NATIVE_MODE: u32 = 0x0200_0000;

#[derive(Default)]
pub(super) struct UiScale {
    panel: Option<Panel>,
}

struct Panel {
    display: DisplayId,
    logical: Size<Pixels>,
    scale: Option<f32>,
}

impl UiScale {
    pub(super) fn for_window(&mut self, window: &Window, cx: &App) -> f32 {
        let backing = window.scale_factor();
        // HiDPI UI is drawn into the same Retina backing surface as the video.
        if backing > 1. {
            return backing;
        }
        let Some(display) = window.display(cx) else {
            return backing;
        };
        let id = display.id();
        let logical = display.bounds().size;
        if let Some(panel) = &self.panel {
            if panel.display == id && panel.logical == logical {
                return panel.scale.unwrap_or(backing);
            }
        }
        let scale = native_ui_scale(id.into(), logical);
        self.panel = Some(Panel {
            display: id,
            logical,
            scale,
        });
        scale.unwrap_or(backing)
    }
}

fn native_ui_scale(display: u32, logical: Size<Pixels>) -> Option<f32> {
    // Core Graphics documents the returned array as CGDisplayMode objects.
    let modes: CFRetained<CFArray<CGDisplayMode>> =
        unsafe { CFRetained::cast_unchecked(CGDisplayCopyAllDisplayModes(display, None)?) };
    let logical = (f32::from(logical.width), f32::from(logical.height));
    modes
        .iter()
        .filter(|mode| CGDisplayMode::io_flags(Some(mode)) & NATIVE_MODE != 0)
        .filter_map(|mode| {
            panel_scale(
                (
                    CGDisplayMode::pixel_width(Some(&mode)) as f32,
                    CGDisplayMode::pixel_height(Some(&mode)) as f32,
                ),
                logical,
            )
        })
        .max_by(f32::total_cmp)
}

fn panel_scale(native: (f32, f32), logical: (f32, f32)) -> Option<f32> {
    if logical.0 <= 0. || logical.1 <= 0. {
        return None;
    }
    // Compare long and short edges so a rotated monitor uses the same scale.
    let long = native.0.max(native.1) / logical.0.max(logical.1);
    let short = native.0.min(native.1) / logical.0.min(logical.1);
    let scale = long.min(short);
    // A different aspect ratio can mean letterboxing, with no uniform scale.
    (scale >= 1. && (long - short).abs() <= scale * 0.02).then_some(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_fractional_scaling_for_a_non_retina_mode() {
        assert_eq!(panel_scale((2560., 2880.), (1600., 1800.)), Some(1.6));
        assert_eq!(panel_scale((2560., 1440.), (960., 1706.)), Some(1.5));
    }

    #[test]
    fn rejects_letterboxed_modes_and_invalid_dimensions() {
        assert_eq!(panel_scale((2560., 1440.), (1024., 768.)), None);
        assert_eq!(panel_scale((2560., 1440.), (0., 0.)), None);
        assert_eq!(panel_scale((2560., 1440.), (3200., 1800.)), None);
    }
}
