//! AVFoundation capture and VideoToolbox H.264 for the camera (`ARCH-038`).
//!
//! **macOS 13 launch safety** (M-review change 7): nothing here references a symbol newer
//! than macOS 13. The 14+ device types (`External`, `ContinuityCamera`) are passed by their
//! string values, chosen by an `NSProcessInfo` version check, so no extern static links
//! them; `systemPreferredCamera` (14+) is sent only after `respondsToSelector:`. Every
//! VideoToolbox key used exists on 13 (the newest, `EnableLowLatencyRateControl`, is 11.3).
//!
//! **TCC**: nothing lists or opens a camera unless `authorizationStatusForMediaType:` (which
//! never prompts) already says Authorized.
//!
//! **Ownership** (M-review change 8): the serial delegate queue is the only owner of the
//! `VTCompressionSession`. It creates, encodes, resizes and invalidates it. Teardown on
//! `skerry-camera`: `stopRunning`, detach the delegate, then `dispatch_sync` onto the queue to
//! complete and invalidate the encoder and mark the pipeline closed, then drop the session.
//! The VT output callback's refcon is an `Arc` released only after `Invalidate` returns.

use super::{avcc_to_annexb, pick_format, CamBackend, Health, KeyframeGate, Uplink};
use super::{BITRATE, FRAME_RATE, KEYFRAME_EVERY};
use crate::capture::{self, Auth, Subject};
use crate::input_devices::InputList;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, ClassType, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceInput,
    AVCaptureDevicePosition, AVCaptureOutput, AVCaptureSession, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_H264, CMSampleBuffer, CMTime,
    CMVideoFormatDescriptionGetDimensions, CMVideoFormatDescriptionGetH264ParameterSetAtIndex,
};
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVImageBuffer, CVPixelBufferGetHeight,
    CVPixelBufferGetWidth,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSNumber, NSOperatingSystemVersion, NSProcessInfo, NSString,
};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate,
    kVTCompressionPropertyKey_ExpectedFrameRate,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel,
    kVTCompressionPropertyKey_RealTime, kVTEncodeFrameOptionKey_ForceKeyFrame,
    kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
    kVTVideoEncoderSpecification_EnableLowLatencyRateControl, VTCompressionSession,
    VTEncodeInfoFlags, VTSessionSetProperty,
};
use std::ffi::{c_void, CStr};
use std::ptr::{null, null_mut, NonNull};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The device type constants, resolved at run time with `dlsym` so no 14+ extern static is
/// linked (`device_type_names_match_the_sdk_constants` checks them against the SDK).
const WIDE_ANGLE: &CStr = c"AVCaptureDeviceTypeBuiltInWideAngleCamera";
const EXTERNAL: &CStr = c"AVCaptureDeviceTypeExternal";
const CONTINUITY: &CStr = c"AVCaptureDeviceTypeContinuityCamera";
const EXTERNAL_UNKNOWN: &CStr = c"AVCaptureDeviceTypeExternalUnknown";
/// `kCVPixelBufferPixelFormatTypeKey`'s value.
const PIXEL_FORMAT_KEY: &str = "PixelFormatType";

fn at_least_sonoma() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 14,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// An `NSString *` constant exported by a loaded framework (AVFoundation is linked), or
/// `None` when this macOS doesn't have it.
fn framework_string(name: &CStr) -> Option<Retained<NSString>> {
    // SAFETY: a lookup in the loaded images; the symbol, if any, is an `NSString *const`.
    let slot = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }.cast::<*const NSString>();
    // SAFETY: as above; framework constants are immortal.
    let value = unsafe { *slot.as_ref()? };
    // SAFETY: a valid (or null) object pointer; retaining an immortal constant is harmless.
    unsafe { Retained::retain(value.cast_mut()) }
}

/// The camera kinds to discover on this macOS.
fn device_types(sonoma: bool) -> &'static [&'static CStr] {
    if sonoma {
        &[WIDE_ANGLE, EXTERNAL, CONTINUITY]
    } else {
        &[WIDE_ANGLE, EXTERNAL_UNKNOWN]
    }
}

fn authorized() -> bool {
    crate::permissions::authorization(Subject::Camera) == Auth::Authorized
}

fn discover() -> Vec<Retained<AVCaptureDevice>> {
    let names: Vec<Retained<NSString>> = device_types(at_least_sonoma())
        .iter()
        .filter_map(|name| framework_string(name))
        .collect();
    let types = NSArray::from_retained_slice(&names);
    // SAFETY: device types valid on this macOS (checked above) and the video media type.
    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            AVMediaTypeVideo,
            AVCaptureDevicePosition::Unspecified,
        )
    };
    // SAFETY: a plain property read.
    unsafe { session.devices() }.to_vec()
}

fn default_camera() -> Option<Retained<AVCaptureDevice>> {
    let class = AVCaptureDevice::class();
    if class.responds_to(sel!(systemPreferredCamera)) {
        // SAFETY: the selector exists here (macOS 14+) and returns an optional device.
        let preferred: Option<Retained<AVCaptureDevice>> =
            unsafe { msg_send![class, systemPreferredCamera] };
        if preferred.is_some() {
            return preferred;
        }
    }
    // SAFETY: the video media type (10.7+).
    unsafe { AVCaptureDevice::defaultDeviceWithMediaType(AVMediaTypeVideo?) }
}

/// The connected cameras as `(unique ID, name)` and the default's ID, for the card.
/// `None` until macOS granted the camera: listing never runs before that.
pub fn list_cameras() -> Option<InputList> {
    if !authorized() {
        return None;
    }
    let devices = discover()
        .iter()
        .map(|device| {
            // SAFETY: plain property reads.
            let (uid, name) = unsafe { (device.uniqueID(), device.localizedName()) };
            (uid.to_string(), Some(name.to_string()))
        })
        .collect();
    // SAFETY: a plain property read.
    let default_uid = default_camera().map(|device| unsafe { device.uniqueID() }.to_string());
    Some(InputList {
        devices,
        default_uid,
    })
}

/// AVFoundation, as the engine's backend.
#[derive(Default)]
pub(super) struct AvfCamera {
    default_uid: Option<String>,
}

impl CamBackend for AvfCamera {
    type Session = Session;

    fn devices(&mut self) -> Option<Vec<(String, Option<String>)>> {
        let list = list_cameras()?;
        self.default_uid = list.default_uid;
        Some(list.devices)
    }

    fn default_uid(&mut self) -> Option<String> {
        self.default_uid.clone()
    }

    fn encoder_ok(&mut self) -> bool {
        Encoder::new(640, 480, |_, _| {}).is_ok()
    }

    fn open(&mut self, uid: &str, size: (u32, u32), uplink: Uplink) -> Result<Session, Health> {
        if !authorized() {
            return Err(Health::Stopped);
        }
        Session::open(uid, size, uplink)
    }

    fn health(&mut self, session: &Session) -> Health {
        session.health()
    }
}

/// The CF types the pipeline holds, used only under its mutex on the delegate queue.
struct Owned<T>(T);
// SAFETY: the compression session and option dictionaries are only touched while the
// pipeline's mutex is held (on the delegate queue, or the teardown's `dispatch_sync` onto
// it). VideoToolbox sessions may be used from any thread one call at a time.
unsafe impl<T> Send for Owned<T> {}

/// What the VT output callback reaches through its refcon.
struct EncoderOutput {
    closed: AtomicBool,
    deliver: Box<dyn Fn(bool, Vec<u8>) + Send + Sync>,
}

/// One hardware H.264 compression session: Constrained Baseline, real time, no reordering.
pub(super) struct Encoder {
    session: Owned<CFRetained<VTCompressionSession>>,
    force_key: Owned<CFRetained<CFDictionary<CFString, CFBoolean>>>,
    /// `Arc::into_raw` of the callback's state, released after `Invalidate` returns.
    output: *const EncoderOutput,
    size: (usize, usize),
}

// SAFETY: see `Owned`; `output` is an `Arc` whose target is `Sync`.
unsafe impl Send for Encoder {}

fn set(session: &VTCompressionSession, key: &CFString, value: &CFType) -> bool {
    // SAFETY: a valid session, a framework key and a value of the key's type.
    unsafe { VTSessionSetProperty(session, key, Some(value)) == 0 }
}

impl Encoder {
    /// `deliver(keyframe, annexb)` gets each encoded frame, on a VideoToolbox thread.
    pub(super) fn new(
        width: usize,
        height: usize,
        deliver: impl Fn(bool, Vec<u8>) + Send + Sync + 'static,
    ) -> Result<Self, ()> {
        let output = Arc::into_raw(Arc::new(EncoderOutput {
            closed: AtomicBool::new(false),
            deliver: Box::new(deliver),
        }));
        let yes = CFBoolean::new(true);
        let no = CFBoolean::new(false);
        let mut session = null_mut();
        // SAFETY: framework keys, a callback matching VTCompressionOutputCallback, and a
        // refcon that stays valid until after Invalidate (see Drop).
        let status = unsafe {
            let spec = CFDictionary::<CFString, CFBoolean>::from_slices(
                &[
                    kVTVideoEncoderSpecification_EnableLowLatencyRateControl,
                    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
                ],
                &[yes, yes],
            );
            VTCompressionSession::create(
                None,
                width as i32,
                height as i32,
                kCMVideoCodecType_H264,
                Some(spec.as_opaque()),
                None,
                None,
                Some(encoded),
                output.cast_mut().cast(),
                NonNull::from(&mut session),
            )
        };
        let Some(session) = NonNull::new(session).filter(|_| status == 0) else {
            // SAFETY: the Arc from above, never handed to a live session.
            drop(unsafe { Arc::from_raw(output) });
            return Err(());
        };
        // SAFETY: a +1 session from the Create call.
        let session = unsafe { CFRetained::from_raw(session) };
        let bitrate = CFNumber::new_i32(BITRATE as i32);
        let rate = CFNumber::new_i32(FRAME_RATE as i32);
        let interval = CFNumber::new_f64(KEYFRAME_EVERY.as_secs_f64());
        // SAFETY: framework constants (all macOS 12 or older).
        let configured = unsafe {
            set(&session, kVTCompressionPropertyKey_RealTime, yes)
                & set(&session, kVTCompressionPropertyKey_AllowFrameReordering, no)
                & set(
                    &session,
                    kVTCompressionPropertyKey_ProfileLevel,
                    kVTProfileLevel_H264_ConstrainedBaseline_AutoLevel,
                )
                & set(&session, kVTCompressionPropertyKey_AverageBitRate, &bitrate)
                & set(&session, kVTCompressionPropertyKey_ExpectedFrameRate, &rate)
                & set(
                    &session,
                    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration,
                    &interval,
                )
        };
        // SAFETY: a valid session.
        let prepared = unsafe { session.prepare_to_encode_frames() } == 0;
        // SAFETY: framework constants.
        let force_key = unsafe {
            CFDictionary::<CFString, CFBoolean>::from_slices(
                &[kVTEncodeFrameOptionKey_ForceKeyFrame],
                &[yes],
            )
        };
        let encoder = Self {
            session: Owned(session),
            force_key: Owned(force_key),
            output,
            size: (width, height),
        };
        // The profile is what the server expects; without it, give up (Drop invalidates).
        if !configured || !prepared {
            return Err(());
        }
        Ok(encoder)
    }

    /// Queues one frame; the output arrives through `deliver`.
    pub(super) fn encode(&self, image: &CVImageBuffer, pts: CMTime, key: bool) -> Result<(), ()> {
        let options = key.then(|| self.force_key.0.as_opaque());
        // SAFETY: a valid session and image buffer; VT retains what it keeps.
        let status = unsafe {
            self.session
                .0
                .encode_frame(image, pts, kCMTimeInvalid, options, null_mut(), null_mut())
        };
        if status == 0 {
            Ok(())
        } else {
            Err(())
        }
    }

    /// Emits every queued frame before returning.
    pub(super) fn flush(&self) {
        // SAFETY: a valid session; an invalid time completes everything.
        unsafe { self.session.0.complete_frames(kCMTimeInvalid) };
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.flush();
        // SAFETY: `output` is the live Arc from `new`.
        unsafe { &*self.output }
            .closed
            .store(true, Ordering::Release);
        // SAFETY: after Invalidate returns, VT calls the output callback no more.
        unsafe { self.session.0.invalidate() };
        // SAFETY: the Arc from `new`, released exactly once, after Invalidate.
        drop(unsafe { Arc::from_raw(self.output) });
    }
}

/// The parameter set at `index` of an H.264 format description, and the NAL length size.
fn parameter_set(
    format: &objc2_core_media::CMFormatDescription,
    index: usize,
) -> Option<(Vec<u8>, usize)> {
    let mut pointer: *const u8 = null();
    let mut size = 0usize;
    let mut nal_len = 0i32;
    // SAFETY: out pointers to locals; the bytes are copied while `format` is retained.
    let status = unsafe {
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
            format,
            index,
            &mut pointer,
            &mut size,
            null_mut(),
            &mut nal_len,
        )
    };
    if status != 0 || pointer.is_null() {
        return None;
    }
    // SAFETY: VT points at `size` bytes inside the retained format description.
    let set = unsafe { std::slice::from_raw_parts(pointer, size) }.to_vec();
    Some((set, usize::try_from(nal_len).ok()?))
}

/// Whether an encoded sample is a sync (key) frame: `NotSync` absent or false.
fn is_sync(sample: &CMSampleBuffer) -> bool {
    // SAFETY: reads the existing attachments without creating them.
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return true;
    };
    if attachments.count() == 0 {
        return true;
    }
    // SAFETY: index 0 exists; each element is a CFDictionary.
    let first = unsafe { attachments.value_at_index(0) }.cast::<CFDictionary>();
    let Some(first) = (unsafe { first.as_ref() }) else {
        return true;
    };
    // SAFETY: a CFString key; the value, if any, is a CFBoolean.
    let value = unsafe { first.value((kCMSampleAttachmentKey_NotSync as *const CFString).cast()) };
    !std::ptr::eq(
        value.cast::<CFBoolean>(),
        CFBoolean::new(true) as *const CFBoolean,
    )
}

/// One compressed sample as Annex B, with SPS/PPS first on a keyframe.
fn annexb(sample: &CMSampleBuffer) -> Option<(bool, Vec<u8>)> {
    // SAFETY: plain reads of a valid sample buffer.
    let (data, format) = unsafe { (sample.data_buffer()?, sample.format_description()?) };
    // SAFETY: as above.
    let len = unsafe { data.data_length() };
    let mut bytes = vec![0u8; len];
    if len > 0 {
        // SAFETY: `bytes` holds `len` bytes.
        let status = unsafe {
            data.copy_data_bytes(0, len, NonNull::new(bytes.as_mut_ptr().cast::<c_void>())?)
        };
        if status != 0 {
            return None;
        }
    }
    let key = is_sync(sample);
    let (sps, nal_len) = parameter_set(&format, 0)?;
    let params = if key {
        let (pps, _) = parameter_set(&format, 1)?;
        Some((sps, pps))
    } else {
        None
    };
    let frame = avcc_to_annexb(
        &bytes,
        nal_len,
        params
            .as_ref()
            .map(|(sps, pps)| (sps.as_slice(), pps.as_slice())),
    )?;
    Some((key, frame))
}

/// VideoToolbox's output callback, on one of its threads.
unsafe extern "C-unwind" fn encoded(
    refcon: *mut c_void,
    _: *mut c_void,
    status: i32,
    flags: VTEncodeInfoFlags,
    sample: *mut CMSampleBuffer,
) {
    // SAFETY: the refcon is the encoder's Arc, alive until after Invalidate.
    let Some(output) = (unsafe { refcon.cast::<EncoderOutput>().as_ref() }) else {
        return;
    };
    if status != 0 || flags.contains(VTEncodeInfoFlags::FrameDropped) {
        return;
    }
    if output.closed.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: VT passes a valid sample (or null) for the callback's duration.
    let Some(sample) = (unsafe { sample.as_ref() }) else {
        return;
    };
    if let Some((key, frame)) = annexb(sample) {
        (output.deliver)(key, frame);
    }
}

/// The delegate queue's state: the encoder, the keyframe gate and the frame pacing.
pub(super) struct Pipeline {
    uplink: Uplink,
    encoder: Option<Encoder>,
    gate: KeyframeGate,
    last_pts: Option<f64>,
    closed: bool,
    /// The encoder can't be made or keeps failing: the session counts as stopped.
    failed: Arc<AtomicBool>,
}

impl Pipeline {
    pub(super) fn new(uplink: Uplink) -> Self {
        Self {
            uplink,
            encoder: None,
            gate: KeyframeGate::default(),
            last_pts: None,
            closed: false,
            failed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// One captured frame (delegate queue only).
    pub(super) fn push(&mut self, image: &CVImageBuffer, pts: CMTime, now: Instant) {
        if self.closed {
            return;
        }
        // Admit at 30 fps by timestamp, for cameras that ignore the frame duration.
        // SAFETY: a plain conversion of a CMTime.
        let seconds = (pts.flags.contains(objc2_core_media::CMTimeFlags::Valid))
            .then(|| unsafe { pts.seconds() });
        if let (Some(at), Some(last)) = (seconds, self.last_pts) {
            if at - last < 1.0 / f64::from(FRAME_RATE) - 0.004 {
                return;
            }
        }
        let size = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
        if self
            .encoder
            .as_ref()
            .is_none_or(|encoder| encoder.size != size)
        {
            self.encoder = None;
            let uplink = self.uplink.clone();
            match Encoder::new(size.0, size.1, move |key, frame| {
                uplink.send(capture::webcam_frame(key, &frame));
            }) {
                Ok(encoder) => {
                    self.encoder = Some(encoder);
                    self.gate = KeyframeGate::default();
                }
                Err(()) => {
                    self.failed.store(true, Ordering::Release);
                    return;
                }
            }
        }
        if self.uplink.take_keyframe_request() {
            self.gate.request();
        }
        let key = self.gate.take(now);
        let Some(encoder) = &self.encoder else {
            return;
        };
        if encoder.encode(image, pts, key).is_err() {
            self.encoder = None;
            self.failed.store(true, Ordering::Release);
            return;
        }
        if seconds.is_some() {
            self.last_pts = seconds;
        }
    }

    /// Completes and invalidates the encoder; later frames are ignored (delegate queue only).
    pub(super) fn shutdown(&mut self) {
        self.closed = true;
        self.encoder = None;
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "SkerryCameraDelegate"]
    #[ivars = Arc<Mutex<Pipeline>>]
    struct CameraDelegate;

    unsafe impl NSObjectProtocol for CameraDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(
            &self,
            _output: &AVCaptureOutput,
            sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // SAFETY: plain reads of the sample AVFoundation hands over.
            let (image, pts) = unsafe { (sample.image_buffer(), sample.presentation_time_stamp()) };
            let Some(image) = image else {
                return;
            };
            if let Ok(mut pipeline) = self.ivars().lock() {
                pipeline.push(&image, pts, Instant::now());
            }
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        fn did_drop(
            &self,
            _output: &AVCaptureOutput,
            _sample: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // A frame late at capture is not a gap in the stream: the next encodes normally.
        }
    }
);

impl CameraDelegate {
    fn new(pipeline: Arc<Mutex<Pipeline>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(pipeline);
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// One running capture session, torn down on `skerry-camera` when dropped.
pub(super) struct Session {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureVideoDataOutput>,
    device: Retained<AVCaptureDevice>,
    _delegate: Retained<CameraDelegate>,
    queue: DispatchRetained<DispatchQueue>,
    pipeline: Arc<Mutex<Pipeline>>,
    failed: Arc<AtomicBool>,
}

type Format = Retained<objc2_av_foundation::AVCaptureDeviceFormat>;

/// `(width, height, max fps)` of each format, and the formats themselves.
fn formats(device: &AVCaptureDevice) -> (Vec<(u32, u32, f64)>, Vec<Format>) {
    // SAFETY: plain property reads.
    let all = unsafe { device.formats() }.to_vec();
    let sizes = all
        .iter()
        .map(|format| {
            // SAFETY: as above.
            let dimensions =
                unsafe { CMVideoFormatDescriptionGetDimensions(&format.formatDescription()) };
            // SAFETY: as above.
            let fps = unsafe { format.videoSupportedFrameRateRanges() }
                .iter()
                .map(|range| unsafe { range.maxFrameRate() })
                .fold(0.0, f64::max);
            (
                dimensions.width.max(0) as u32,
                dimensions.height.max(0) as u32,
                fps,
            )
        })
        .collect();
    (sizes, all)
}

/// Sets the format nearest `size` and 30 fps where the format allows it.
fn configure(device: &AVCaptureDevice, size: (u32, u32)) {
    let (sizes, all) = formats(device);
    let Some(index) = pick_format(&sizes, size) else {
        return;
    };
    // SAFETY: the device is in a session, as setting a format needs.
    if unsafe { device.lockForConfiguration() }.is_err() {
        return;
    }
    let format = &all[index];
    // SAFETY: a format from this device's own list.
    unsafe { device.setActiveFormat(format) };
    let rate = f64::from(FRAME_RATE);
    // SAFETY: plain property reads.
    let supports = unsafe { format.videoSupportedFrameRateRanges() }
        .iter()
        .any(|range| unsafe { range.minFrameRate() <= rate && rate <= range.maxFrameRate() + 0.5 });
    if supports {
        // SAFETY: a duration inside a supported range of the active format.
        unsafe {
            device.setActiveVideoMinFrameDuration(CMTime::new(1, FRAME_RATE as i32));
        }
    }
    // SAFETY: locked above.
    unsafe { device.unlockForConfiguration() };
}

impl Session {
    fn open(uid: &str, size: (u32, u32), uplink: Uplink) -> Result<Self, Health> {
        // SAFETY: a lookup by ID.
        let device = unsafe { AVCaptureDevice::deviceWithUniqueID(&NSString::from_str(uid)) }
            .ok_or(Health::Stopped)?;
        // SAFETY: opens the device for capture; macOS granted the camera (checked by caller).
        let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }.map_err(
            |_| {
                // SAFETY: a plain property read.
                if unsafe { device.isInUseByAnotherApplication() } {
                    Health::InUse
                } else {
                    Health::Stopped
                }
            },
        )?;
        // SAFETY: a new, unshared session configured on this thread only.
        let session = unsafe { AVCaptureSession::new() };
        unsafe { session.beginConfiguration() };
        // SAFETY: as above.
        if !unsafe { session.canAddInput(&input) } {
            unsafe { session.commitConfiguration() };
            return Err(Health::InUse);
        }
        unsafe { session.addInput(&input) };
        configure(&device, size);
        // SAFETY: a new output; the dictionary holds a pixel format number.
        let output = unsafe { AVCaptureVideoDataOutput::new() };
        let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
        let settings = NSDictionary::from_retained_objects(
            &[&*NSString::from_str(PIXEL_FORMAT_KEY)],
            &[Retained::into_super(Retained::into_super(
                Retained::into_super(format),
            ))],
        );
        unsafe {
            output.setVideoSettings(Some(&settings));
            output.setAlwaysDiscardsLateVideoFrames(true);
        }
        let queue = DispatchQueue::new("dev.kebot.skerry.camera", None);
        let pipeline = Pipeline::new(uplink);
        let failed = pipeline.failed.clone();
        let pipeline = Arc::new(Mutex::new(pipeline));
        let delegate = CameraDelegate::new(pipeline.clone());
        // SAFETY: a serial queue and a delegate that outlives the session (held below).
        unsafe {
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
        }
        // SAFETY: as above.
        if !unsafe { session.canAddOutput(&output) } {
            unsafe { session.commitConfiguration() };
            return Err(Health::Stopped);
        }
        unsafe {
            session.addOutput(&output);
            session.commitConfiguration();
            // Blocks until running; only ever on `skerry-camera`.
            session.startRunning();
        }
        Ok(Self {
            session,
            output,
            device,
            _delegate: delegate,
            queue,
            pipeline,
            failed,
        })
    }

    fn health(&self) -> Health {
        // SAFETY: plain property reads.
        let (running, interrupted, in_use) = unsafe {
            (
                self.session.isRunning(),
                self.session.isInterrupted(),
                self.device.isInUseByAnotherApplication(),
            )
        };
        if self.failed.load(Ordering::Acquire) {
            Health::Stopped
        } else if running && !interrupted {
            Health::Running
        } else if interrupted || in_use {
            Health::InUse
        } else {
            Health::Stopped
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: on `skerry-camera`, the thread that started it.
        unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
        }
        // The queue owns the encoder: complete, invalidate and close it there, after any
        // frame already queued.
        let pipeline = self.pipeline.clone();
        self.queue.exec_sync(move || {
            if let Ok(mut pipeline) = pipeline.lock() {
                pipeline.shutdown();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_video::{
        kCVPixelBufferIOSurfacePropertiesKey, CVPixelBuffer, CVPixelBufferCreate,
        CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
        CVPixelBufferGetHeightOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress,
    };
    use std::time::Duration;

    /// An IOSurface-backed NV12 (video range) buffer with a moving gradient.
    fn nv12(width: usize, height: usize, frame: usize) -> CFRetained<CVPixelBuffer> {
        let mut buffer = null_mut();
        // SAFETY: framework constants and an out pointer.
        let status = unsafe {
            let surface = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
            let attributes = CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferIOSurfacePropertiesKey],
                &[surface.as_ref()],
            );
            CVPixelBufferCreate(
                None,
                width,
                height,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                Some(attributes.as_opaque()),
                NonNull::from(&mut buffer),
            )
        };
        assert_eq!(status, 0);
        // SAFETY: a +1 buffer from Create.
        let buffer = unsafe { CFRetained::from_raw(NonNull::new(buffer).unwrap()) };
        let flags = CVPixelBufferLockFlags(0);
        // SAFETY: the planes are writable for the lock's lifetime.
        unsafe {
            assert_eq!(CVPixelBufferLockBaseAddress(&buffer, flags), 0);
            for plane in 0..2 {
                let base = CVPixelBufferGetBaseAddressOfPlane(&buffer, plane).cast::<u8>();
                let stride = CVPixelBufferGetBytesPerRowOfPlane(&buffer, plane);
                let rows = CVPixelBufferGetHeightOfPlane(&buffer, plane);
                for row in 0..rows {
                    for column in 0..stride {
                        let value = if plane == 0 {
                            16 + ((row + column + frame * 7) % 200)
                        } else {
                            128
                        };
                        *base.add(row * stride + column) = value as u8;
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer, flags);
        }
        buffer
    }

    /// The NAL unit types of an Annex B access unit.
    fn nal_types(annexb: &[u8]) -> Vec<u8> {
        let mut types = Vec::new();
        let mut i = 0;
        while i + 3 < annexb.len() {
            if annexb[i..i + 3] == [0, 0, 1] {
                types.push(annexb[i + 3] & 0x1f);
                i += 3;
            } else {
                i += 1;
            }
        }
        types
    }

    #[test]
    fn device_type_names_match_the_sdk_constants() {
        // Test-only references: the app itself never links these 14+ statics.
        let resolved = |name| framework_string(name).unwrap().to_string();
        // SAFETY: immutable framework constants, present on the Mac running the tests.
        unsafe {
            use objc2_av_foundation::AVCaptureDeviceTypeBuiltInWideAngleCamera;
            #[allow(deprecated)]
            use objc2_av_foundation::AVCaptureDeviceTypeExternalUnknown;
            assert_eq!(
                resolved(WIDE_ANGLE),
                AVCaptureDeviceTypeBuiltInWideAngleCamera.to_string()
            );
            #[allow(deprecated)]
            let unknown = AVCaptureDeviceTypeExternalUnknown.to_string();
            assert_eq!(resolved(EXTERNAL_UNKNOWN), unknown);
            if at_least_sonoma() {
                use objc2_av_foundation::{
                    AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal,
                };
                assert_eq!(resolved(EXTERNAL), AVCaptureDeviceTypeExternal.to_string());
                assert_eq!(
                    resolved(CONTINUITY),
                    AVCaptureDeviceTypeContinuityCamera.to_string()
                );
            }
            assert_eq!(
                objc2_core_video::kCVPixelBufferPixelFormatTypeKey.to_string(),
                PIXEL_FORMAT_KEY
            );
        }
        assert!(framework_string(c"SkerryNoSuchConstant").is_none());
        assert_eq!(device_types(false), [WIDE_ANGLE, EXTERNAL_UNKNOWN]);
        assert_eq!(device_types(true), [WIDE_ANGLE, EXTERNAL, CONTINUITY]);
    }

    #[test]
    fn listing_waits_for_the_camera_permission() {
        if !authorized() {
            assert!(list_cameras().is_none());
        }
        // Reading the permission never changes it.
        let before = crate::permissions::authorization(Subject::Camera);
        let _ = list_cameras();
        assert_eq!(crate::permissions::authorization(Subject::Camera), before);
    }

    #[test]
    fn vt_h264_encoder_produces_annexb_keyframe() {
        let frames = Arc::new(Mutex::new(Vec::<(bool, Vec<u8>)>::new()));
        let sink = frames.clone();
        let encoder = Encoder::new(320, 240, move |key, frame| {
            sink.lock().unwrap().push((key, frame));
        })
        .expect("VideoToolbox makes an H.264 session");
        for frame in 0..3 {
            let buffer = nv12(320, 240, frame);
            // SAFETY: a plain CMTime.
            let pts = unsafe { CMTime::new(frame as i64, 30) };
            encoder.encode(&buffer, pts, frame == 0).unwrap();
        }
        encoder.flush();
        drop(encoder);
        let frames = frames.lock().unwrap();
        assert!(!frames.is_empty(), "frames came out");
        let (key, first) = &frames[0];
        assert!(*key, "the first frame is a keyframe");
        assert_eq!(&first[..4], [0, 0, 0, 1]);
        assert_eq!(first[4] & 0x1f, 7, "SPS first");
        let types = nal_types(first);
        assert!(types.contains(&8), "a PPS: {types:?}");
        assert!(types.contains(&5), "an IDR slice: {types:?}");
        // Constrained Baseline: profile_idc 66 with constraint_set1.
        assert_eq!(first[5], 66);
        assert!(first[6] & 0x40 != 0);
        // The 0x06 frame the server gets, and the decoder Skerry itself uses accepts it.
        let wire = capture::webcam_frame(*key, first);
        assert_eq!(&wire[..3], [6, 1, 1]);
        let mut decoder = crate::video::Decoder::default();
        let packet = crate::video::Packet {
            codec: crate::video::Codec::H264,
            id: 0,
            reference: 0,
            size: (320, 240),
            key: true,
            payload: first,
        };
        assert!(decoder.decode(&packet).unwrap().is_some());
        // Later frames are P frames without parameter sets.
        for (key, frame) in frames.iter().skip(1) {
            assert!(!key);
            assert!(!nal_types(frame).contains(&7));
        }
    }

    #[test]
    fn pipeline_teardown_with_frames_in_flight_is_safe() {
        // Twenty start/stop cycles: frames queued on the delegate queue while the teardown's
        // dispatch_sync completes and invalidates the encoder there.
        for round in 0..20 {
            let (sink, mut frames) = tokio::sync::mpsc::channel(64);
            let keyframe = Arc::new(AtomicBool::new(false));
            let pipeline = Arc::new(Mutex::new(Pipeline::new(Uplink::new(sink, keyframe))));
            let queue = DispatchQueue::new("dev.kebot.skerry.camera.test", None);
            for frame in 0..6 {
                let pipeline = pipeline.clone();
                let buffer = Owned(nv12(160, 120, frame));
                queue.exec_async(move || {
                    let buffer = buffer;
                    // SAFETY: a plain CMTime.
                    let pts = unsafe { CMTime::new(frame as i64, 30) };
                    let now = Instant::now() + Duration::from_millis(frame as u64 * 34);
                    pipeline.lock().unwrap().push(&buffer.0, pts, now);
                });
            }
            let closing = pipeline.clone();
            queue.exec_sync(move || closing.lock().unwrap().shutdown());
            // Nothing arrives after the shutdown, and pushes after it are ignored.
            let before = std::iter::from_fn(|| frames.try_recv().ok()).count();
            let late = nv12(160, 120, 9);
            // SAFETY: a plain CMTime.
            let pts = unsafe { CMTime::new(99, 30) };
            pipeline.lock().unwrap().push(&late, pts, Instant::now());
            assert_eq!(frames.try_recv().ok(), None, "round {round}");
            assert!(pipeline.lock().unwrap().encoder.is_none());
            if round == 0 {
                assert!(before >= 1, "frames were in flight");
            }
        }
    }

    #[test]
    fn pipeline_paces_to_30fps_and_honours_keyframe_requests() {
        let (sink, mut frames) = tokio::sync::mpsc::channel(64);
        let keyframe = Arc::new(AtomicBool::new(false));
        let mut pipeline = Pipeline::new(Uplink::new(sink, keyframe.clone()));
        let start = Instant::now();
        // 60 fps in: only every other frame is admitted.
        for frame in 0..12 {
            let buffer = nv12(160, 120, frame);
            // SAFETY: a plain CMTime.
            let pts = unsafe { CMTime::new(frame as i64, 60) };
            if frame == 8 {
                keyframe.store(true, Ordering::Release);
            }
            pipeline.push(
                &buffer,
                pts,
                start + Duration::from_millis(frame as u64 * 300),
            );
        }
        pipeline.shutdown();
        let sent: Vec<Vec<u8>> = std::iter::from_fn(|| frames.try_recv().ok()).collect();
        assert_eq!(sent.len(), 6);
        let keys: Vec<bool> = sent
            .iter()
            .map(|frame| capture::webcam_keyframe(frame))
            .collect();
        assert_eq!(keys, [true, false, false, false, true, false]);
        assert!(!keyframe.load(Ordering::Acquire));
    }
}
