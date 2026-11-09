//! Colour-match the window to the display, as other Mac apps' windows are (macOS).
//!
//! DeckCraft draws sRGB colours, but the window it renders into isn't tagged with a colour space, so
//! macOS sends the values to the display unconverted and mid tones look lighter than in
//! colour-managed apps (#404040 text shows as 64 where a colour-managed window shows 49 on the same
//! screen). Tagging the window sRGB asks AppKit to convert it to the display's profile. A safe call
//! in the `objc2-app-kit` bindings (the workspace forbids unsafe). Ported from WordCraft.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSColorSpace};

/// Tag the app's windows sRGB. False before a window exists or off the main thread.
pub fn tag_window_srgb() -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let windows = NSApplication::sharedApplication(mtm).windows();
    if windows.count() == 0 {
        return false;
    }
    for window in windows.iter() {
        window.setColorSpace(Some(&NSColorSpace::sRGBColorSpace()));
    }
    true
}
