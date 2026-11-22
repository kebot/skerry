//! The macOS print dialog for a printed document (`ARCH-033`): PDFKit's print operation, run
//! as a sheet on the session window.
//!
//! Never `lpr` (no dialog), never Apple Events to Preview (an Automation prompt). The sheet
//! form doesn't block GPUI's run loop. Callers run [`print_pdf`] from a spawned task with no
//! `App` borrow held around it: AppKit may re-enter the run loop while it sets the panel up,
//! as `prompt_for_paths` does in GPUI itself.

use objc2::{AnyThread as _, MainThreadMarker};
use objc2_app_kit::NSWindow;
use objc2_foundation::{NSString, NSURL};
use objc2_pdf_kit::{PDFDocument, PDFPrintScalingMode};
use std::path::Path;

/// What happened when Print… ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintOutcome {
    /// The print sheet is up.
    Shown,
    /// Another sheet is up on the window.
    Busy,
    /// An auto-open found the window no longer key: try again on the next activation.
    NotKey,
    /// PDFKit can't print it (protected or damaged).
    Unreadable,
    /// The window is gone, hidden, or this isn't the main thread.
    NoWindow,
}

/// The job title: the document name without `.pdf` in any case.
pub fn job_title(name: &str) -> &str {
    let cut = name.len().saturating_sub(4);
    match name.get(cut..) {
        Some(ext) if ext.eq_ignore_ascii_case(".pdf") && cut > 0 => &name[..cut],
        _ => name,
    }
}

/// Whether a sheet is up on `window`.
pub fn sheet_attached(window: &NSWindow) -> bool {
    window.attachedSheet().is_some()
}

/// Ends whatever sheet is up on `window`, before the window goes away.
pub fn end_sheet(window: &NSWindow) {
    if let Some(sheet) = window.attachedSheet() {
        window.endSheet(&sheet);
    }
}

/// Opens the print sheet for the PDF at `path` on `window`. The window's state is checked
/// again here, as it may have changed since the call was scheduled. With `require_key` (an
/// auto-open) a window that is no longer key gets nothing.
pub fn print_pdf(window: &NSWindow, path: &Path, title: &str, require_key: bool) -> PrintOutcome {
    let Some(mtm) = MainThreadMarker::new() else {
        return PrintOutcome::NoWindow;
    };
    if !window.isVisible() {
        return PrintOutcome::NoWindow;
    }
    if sheet_attached(window) {
        if !require_key {
            window.makeKeyAndOrderFront(None);
        }
        return PrintOutcome::Busy;
    }
    if require_key && !window.isKeyWindow() {
        return PrintOutcome::NotKey;
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: `url` is a valid file URL; PDFKit returns nil for a file it can't read, which
    // the `Option` carries. Main thread (checked above).
    let Some(document) = (unsafe { PDFDocument::initWithURL(PDFDocument::alloc(), &url) }) else {
        return PrintOutcome::Unreadable;
    };
    // SAFETY: a plain property read on a live document.
    if unsafe { document.isLocked() } {
        return PrintOutcome::Unreadable;
    }
    // SAFETY: `None` asks PDFKit for the shared print info; the scaling mode is one of the
    // framework's constants; `mtm` proves the main thread.
    let operation = unsafe {
        document.printOperationForPrintInfo_scalingMode_autoRotate(
            None,
            PDFPrintScalingMode::PageScaleDownToFit,
            true,
            mtm,
        )
    };
    let Some(operation) = operation else {
        return PrintOutcome::Unreadable;
    };
    operation.setJobTitle(Some(&NSString::from_str(job_title(title))));
    operation.setShowsPrintPanel(true);
    operation.setShowsProgressPanel(true);
    // SAFETY: `window` is live and visible on the main thread; no delegate and no selector
    // are passed, so the null context is never read. The operation retains the document,
    // and AppKit retains the operation while its sheet runs.
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            window,
            None,
            None,
            std::ptr::null_mut(),
        );
    }
    PrintOutcome::Shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_titles_drop_the_pdf_extension() {
        assert_eq!(job_title("Report.pdf"), "Report");
        assert_eq!(job_title("Scan (2).PDF"), "Scan (2)");
        assert_eq!(job_title("notes.txt"), "notes.txt");
        assert_eq!(job_title(".pdf"), ".pdf");
        assert_eq!(job_title("Résumé.pdf"), "Résumé");
        assert_eq!(job_title("é"), "é");
    }
}
