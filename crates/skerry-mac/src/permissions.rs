//! macOS capture permission (`ARCH-038`): read with `authorizationStatusForMediaType:` and
//! asked only at the moment of a start, with `requestAccessForMediaType:completionHandler:`.
//! Safe on any thread; the completion runs on an arbitrary queue and only sends.

use crate::capture::{Auth, Subject};
use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVMediaType, AVMediaTypeAudio, AVMediaTypeVideo,
};
use std::sync::Mutex;

fn media_type(subject: Subject) -> Option<&'static AVMediaType> {
    match subject {
        // SAFETY: an immutable framework constant (macOS 10.7+).
        Subject::Microphone => unsafe { AVMediaTypeAudio },
        // SAFETY: as above.
        Subject::Camera => unsafe { AVMediaTypeVideo },
    }
}

/// What macOS says now. Restricted counts as denied.
pub fn authorization(subject: Subject) -> Auth {
    let Some(media) = media_type(subject) else {
        return Auth::Denied;
    };
    // SAFETY: an audio or video media type, the only kinds this call accepts.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    match status {
        AVAuthorizationStatus::Authorized => Auth::Authorized,
        AVAuthorizationStatus::NotDetermined => Auth::NotDetermined,
        _ => Auth::Denied,
    }
}

/// Shows the macOS prompt (only while NotDetermined) and answers once.
pub fn request(subject: Subject) -> tokio::sync::oneshot::Receiver<bool> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    let Some(media) = media_type(subject) else {
        let _ = answer.send(false);
        return answered;
    };
    let answer = Mutex::new(Some(answer));
    let handler = RcBlock::new(move |granted: Bool| {
        if let Some(answer) = answer.lock().ok().and_then(|mut answer| answer.take()) {
            let _ = answer.send(granted.as_bool());
        }
    });
    // SAFETY: an audio or video media type and a block that only sends on a channel.
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler) };
    answered
}

/// The Privacy & Security pane for `subject`.
pub fn privacy_settings_url(subject: Subject) -> &'static str {
    match subject {
        Subject::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Subject::Camera => "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_url_opens_the_microphone_pane() {
        assert!(privacy_settings_url(Subject::Microphone).ends_with("?Privacy_Microphone"));
        assert!(privacy_settings_url(Subject::Camera).ends_with("?Privacy_Camera"));
    }

    #[test]
    fn authorization_reads_without_prompting() {
        // Any answer is fine; reading must not block or prompt.
        let _ = authorization(Subject::Microphone);
        let _ = authorization(Subject::Camera);
    }
}
