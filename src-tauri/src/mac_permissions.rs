//! macOS microphone-permission bootstrap.
//!
//! cpal drives audio through the CoreAudio HAL, which is **not** the TCC-aware
//! API: it never raises the OS microphone prompt on its own, and on older macOS
//! (e.g. Catalina) the HAL returns an *empty* input-device list until access is
//! granted. That deadlocks the picker — no devices means nothing to select,
//! which means Start is never pressed, which means the prompt never fires, which
//! means the list stays empty. We break the cycle by asking AVFoundation for
//! audio access at launch: `AVCaptureDevice` *is* TCC-aware, so this is what
//! actually presents the prompt. Once the user grants it, the frontend's idle
//! device poll repopulates the picker within a couple of seconds.
//!
//! `NSMicrophoneUsageDescription` (`Info.plist`) supplies the prompt copy and is
//! mandatory — without it the OS aborts the process when access is requested.

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

/// Request microphone access if the user hasn't decided yet. A no-op when access
/// is already granted (enumeration works) or already denied/restricted (only the
/// user can change that in System Settings → Privacy & Security → Microphone).
/// Safe to call once at startup, from any thread.
pub fn request_microphone_access() {
    // SAFETY: `AVMediaTypeAudio` is a framework-owned constant `NSString`, and
    // these `AVCaptureDevice` class methods are the documented permission API;
    // neither takes ownership of our arguments. The completion handler is only a
    // no-op kept alive for the duration of the (asynchronous) call.
    unsafe {
        let media_type =
            AVMediaTypeAudio.expect("AVMediaTypeAudio is a non-null framework constant");
        if AVCaptureDevice::authorizationStatusForMediaType(media_type)
            == AVAuthorizationStatus::NotDetermined
        {
            // Presents the prompt asynchronously; the handler runs on an
            // arbitrary queue once the user responds. We don't need its result —
            // the idle device poll surfaces the now-visible devices on its own.
            let handler = RcBlock::new(|_granted: Bool| {});
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler);
        }
    }
}
