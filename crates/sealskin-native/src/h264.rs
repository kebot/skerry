use core_foundation::base::TCFType;
use core_video::pixel_buffer::CVPixelBuffer;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMBlockBufferAssureMemoryNowFlag, kCMVideoCodecType_H264, CMBlockBuffer, CMFormatDescription,
    CMSampleBuffer, CMTime, CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::{
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    CVImageBuffer, CVPixelBufferGetIOSurface,
};
use objc2_video_toolbox::{
    kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder, VTDecodeFrameFlags,
    VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord, VTDecompressionSession,
    VTIsHardwareDecodeSupported,
};
use std::{
    ffi::c_void,
    ptr::{null, null_mut, NonNull},
};

pub struct Packet<'a> {
    pub id: u16,
    pub reference: u16,
    pub size: (u16, u16),
    pub key: bool,
    pub payload: &'a [u8],
}

impl<'a> Packet<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let (header, payload) = bytes.split_first_chunk::<12>()?;
        let [4, kind, id_hi, id_lo, 0, 0, w_hi, w_lo, h_hi, h_lo, ref_hi, ref_lo] = *header else {
            return None;
        };
        // Legacy Selkies used a zero codec nibble for H.264.
        if !matches!(kind >> 4, 0 | 1) || payload.is_empty() {
            return None;
        }
        Some(Self {
            id: u16::from_be_bytes([id_hi, id_lo]),
            reference: u16::from_be_bytes([ref_hi, ref_lo]),
            size: (
                u16::from_be_bytes([w_hi, w_lo]),
                u16::from_be_bytes([h_hi, h_lo]),
            ),
            key: kind & 0xf == 1,
            payload,
        })
    }
}

struct AccessUnit<'a> {
    sps: Option<&'a [u8]>,
    pps: Option<&'a [u8]>,
    avcc: Vec<u8>,
}

impl<'a> AccessUnit<'a> {
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        let mut unit = Self {
            sps: None,
            pps: None,
            avcc: Vec::with_capacity(bytes.len()),
        };
        let start = |bytes: &[u8]| bytes.windows(3).position(|window| window == [0, 0, 1]);
        let offset = start(bytes)?;
        if bytes[..offset].iter().any(|&byte| byte != 0) {
            return None;
        }
        let mut remaining = &bytes[offset + 3..];
        loop {
            let next = start(remaining);
            let mut nalu = &remaining[..next.unwrap_or(remaining.len())];
            while let Some((0, rest)) = nalu.split_last() {
                nalu = rest;
            }
            match nalu.first()? & 0x1f {
                7 => unit.sps = Some(nalu),
                8 => unit.pps = Some(nalu),
                _ => {
                    unit.avcc
                        .extend_from_slice(&u32::try_from(nalu.len()).ok()?.to_be_bytes());
                    unit.avcc.extend_from_slice(nalu);
                }
            }
            let Some(next) = next else { break };
            remaining = &remaining[next + 3..];
        }
        Some(unit)
    }
}

#[derive(Clone)]
pub struct Surface(CVPixelBuffer);

// VideoToolbox has finished writing before publication. The retained buffer is
// only shared for Metal reads, and Core Foundation reference counting is atomic.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    pub fn pixel_buffer(&self) -> CVPixelBuffer {
        self.0.clone()
    }
}

pub fn supported() -> bool {
    // The query takes a constant Core Media codec identifier.
    unsafe { VTIsHardwareDecodeSupported(kCMVideoCodecType_H264) }
}

struct Session {
    session: CFRetained<VTDecompressionSession>,
    format: CFRetained<CMFormatDescription>,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

// Each session is exclusively owned by one task. VideoToolbox sessions have no
// thread affinity, and this decoder uses synchronous submit and output.
unsafe impl Send for Session {}

impl Session {
    fn new(sps: &[u8], pps: &[u8], size: (u16, u16)) -> Result<Self, &'static str> {
        let parameters = [NonNull::from(&sps[0]), NonNull::from(&pps[0])];
        let lengths = [sps.len(), pps.len()];
        let mut format = null();
        // The arrays describe two parameter sets that remain live for the call.
        let status = unsafe {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                2,
                NonNull::from(&parameters[0]),
                NonNull::from(&lengths[0]),
                4,
                NonNull::from(&mut format),
            )
        };
        let format = unsafe { take_created(status, format.cast_mut()) }
            .ok_or("Invalid H.264 parameter sets.")?;
        // Verify the actual coded size before asking VideoToolbox to allocate surfaces.
        let coded = unsafe { CMVideoFormatDescriptionGetDimensions(&format) };
        if (coded.width, coded.height) != (i32::from(size.0), i32::from(size.1)) {
            return Err("H.264 parameter sets disagree with the display size.");
        }

        let format_code = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i32);
        let io_surface = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let yes = CFBoolean::new(true);
        let mut session = null_mut();
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(decoded_frame),
            decompressionOutputRefCon: null_mut(),
        };
        // Framework constants are immutable CFStrings. VideoToolbox retains the
        // configuration and copies the callback record during creation.
        let status = unsafe {
            let hardware = CFDictionary::from_slices(
                &[kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder],
                &[yes],
            );
            let output = CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferPixelFormatTypeKey,
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[format_code.as_ref(), io_surface.as_ref(), yes.as_ref()],
            );
            VTDecompressionSession::create(
                None,
                &format,
                Some(hardware.as_opaque()),
                Some(output.as_opaque()),
                &callback,
                NonNull::from(&mut session),
            )
        };
        let session = unsafe { take_created(status, session) }
            .ok_or("Couldn’t start the hardware H.264 decoder.")?;
        Ok(Self {
            session,
            format,
            sps: sps.to_vec(),
            pps: pps.to_vec(),
        })
    }

    fn decode(&self, bytes: &[u8]) -> Result<Surface, &'static str> {
        let mut block = null_mut();
        let len = bytes.len();
        // Core Media owns the sample bytes, including if VideoToolbox retains a reference.
        let status = unsafe {
            CMBlockBuffer::create_with_memory_block(
                None,
                null_mut(),
                len,
                None,
                null(),
                0,
                len,
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block),
            )
        };
        let block =
            unsafe { take_created(status, block) }.ok_or("Couldn’t allocate the H.264 sample.")?;
        if unsafe {
            CMBlockBuffer::replace_data_bytes(NonNull::from(&bytes[0]).cast(), &block, 0, len)
        } != 0
        {
            return Err("Couldn’t fill the H.264 sample.");
        }
        let mut sample = null_mut();
        let status = unsafe {
            CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(&self.format),
                1,
                0,
                null(),
                1,
                &len,
                NonNull::from(&mut sample),
            )
        };
        let sample =
            unsafe { take_created(status, sample) }.ok_or("Couldn’t create the H.264 sample.")?;
        let mut output: Option<Surface> = None;
        // Without asynchronous or temporal flags, the callback completes before the call returns.
        let status = unsafe {
            self.session.decode_frame(
                &sample,
                VTDecodeFrameFlags::empty(),
                (&mut output as *mut Option<Surface>).cast(),
                null_mut(),
            )
        };
        if status != 0 {
            return Err("Hardware H.264 decode failed.");
        }
        output.ok_or("The H.264 decoder returned no renderable surface.")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // No decode is in flight when the owning task replaces or drops the session.
        unsafe { self.session.invalidate() };
    }
}

// On success, result must be the owned reference returned by a Core Foundation Create call.
unsafe fn take_created<T: objc2_core_foundation::Type>(
    status: i32,
    result: *mut T,
) -> Option<CFRetained<T>> {
    if status != 0 {
        return None;
    }
    NonNull::new(result).map(|ptr| unsafe { CFRetained::from_raw(ptr) })
}

unsafe extern "C-unwind" fn decoded_frame(
    _: *mut c_void,
    output: *mut c_void,
    status: i32,
    _: VTDecodeInfoFlags,
    image: *mut CVImageBuffer,
    _: CMTime,
    _: CMTime,
) {
    if status != 0 || output.is_null() {
        return;
    }
    // The synchronous callback borrows the decoder's pixel buffer and the stack output slot.
    let Some(image) = (unsafe { image.as_ref() }) else {
        return;
    };
    if CVPixelBufferGetIOSurface(Some(image)).is_none() {
        return;
    }
    // Retain a reference through the Core Video bindings that GPUI uses.
    let buffer =
        unsafe { CVPixelBuffer::wrap_under_get_rule(NonNull::from(image).as_ptr().cast()) };
    if buffer.get_pixel_format() == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
        unsafe {
            *output.cast::<Option<Surface>>() = Some(Surface(buffer));
        }
    }
}

#[derive(Default)]
pub struct Decoder {
    session: Option<Session>,
    last: Option<(u16, (u16, u16))>,
}

impl Decoder {
    pub fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Surface>, &'static str> {
        if !packet.key && self.last != Some((packet.reference, packet.size)) {
            self.last = None;
            return Ok(None);
        }
        self.last = None;
        let unit = AccessUnit::parse(packet.payload).ok_or("Invalid Annex B H.264 access unit.")?;
        if packet.key {
            let sps = unit.sps.ok_or("The H.264 keyframe contains no SPS.")?;
            let pps = unit.pps.ok_or("The H.264 keyframe contains no PPS.")?;
            if !self
                .session
                .as_ref()
                .is_some_and(|session| session.sps == sps && session.pps == pps)
            {
                self.session = Some(Session::new(sps, pps, packet.size)?);
            }
        }
        if unit.avcc.is_empty() {
            return Err("The H.264 packet contains no picture data.");
        }
        let session = self
            .session
            .as_ref()
            .ok_or("The H.264 decoder needs a keyframe.")?;
        let surface = session.decode(&unit.avcc)?;
        if (surface.0.get_width(), surface.0.get_height())
            != (usize::from(packet.size.0), usize::from(packet.size.1))
        {
            return Err("The H.264 picture size doesn’t match the packet.");
        }
        self.last = Some((packet.id, packet.size));
        Ok(Some(surface))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_full_frame_h264_with_a_wrapped_reference_id() {
        let bytes = [
            4, 0x10, 0, 0, 0, 0, 5, 0, 2, 208, 255, 255, 0, 0, 1, 0x41, 7,
        ];
        let frame = Packet::parse(&bytes).unwrap();
        assert_eq!(frame.id, 0);
        assert_eq!(frame.reference, 65535);
        assert_eq!(frame.size, (1280, 720));
        assert!(!frame.key);
        assert_eq!(frame.payload, &[0, 0, 1, 0x41, 7]);
        let mut stripe = bytes;
        stripe[5] = 8;
        assert!(Packet::parse(&stripe).is_none());
        let mut vp8 = bytes;
        vp8[1] = 0x21;
        assert!(Packet::parse(&vp8).is_none());
        assert!(Packet::parse(&bytes[..12]).is_none());
    }

    #[test]
    fn hardware_decoder_expands_limited_range_and_recovers_at_a_keyframe() {
        if !supported() {
            return;
        }
        // x264 Baseline Annex B, 318×190, limited range. Black above row 96, white below it.
        let key = [
            0x00, 0x00, 0x00, 0x01, 0x09, 0x10, 0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0xc0, 0x0c,
            0xd9, 0x01, 0x41, 0x9e, 0xab, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00,
            0x03, 0x03, 0x28, 0xf1, 0x42, 0xa4, 0x80, 0x00, 0x00, 0x00, 0x01, 0x68, 0xcb, 0x83,
            0xcb, 0x20, 0x00, 0x00, 0x01, 0x65, 0x88, 0x84, 0x04, 0xaf, 0x26, 0x28, 0x00, 0x0e,
            0x28, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9,
            0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xc9, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d,
            0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7,
            0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75,
            0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xe0, 0x00, 0x00, 0x01, 0x65,
            0x02, 0x88, 0x88, 0x40, 0x4a, 0xf2, 0x62, 0x80, 0x00, 0xe2, 0x8c, 0x9c, 0x9c, 0x9c,
            0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c, 0x9c,
            0x9c, 0x9d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d,
            0x75, 0xd7, 0x5f, 0x1f, 0xff, 0xe1, 0xe8, 0xa0, 0x00, 0x2b, 0xe9, 0xef, 0xbe, 0xfb,
            0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfb, 0xef, 0xbe, 0xfa, 0xeb, 0xae,
            0xba, 0xeb, 0xae, 0xba, 0xeb, 0xae, 0xba, 0xeb, 0xae, 0xba, 0xeb, 0xae, 0xbc, 0x00,
            0x00, 0x01, 0x65, 0x01, 0x42, 0x22, 0x10, 0x12, 0xbc, 0x98, 0xa0, 0x00, 0x37, 0x85,
            0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27, 0x27,
            0x27, 0x27, 0x27, 0x27, 0x27, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7,
            0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75,
            0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d,
            0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x5d, 0x75, 0xd7, 0x80,
        ];
        let delta = [
            0x00, 0x00, 0x00, 0x01, 0x09, 0x30, 0x00, 0x00, 0x01, 0x41, 0x9a, 0x38, 0x09, 0xde,
            0x05, 0x18, 0x00, 0x00, 0x01, 0x41, 0x02, 0x89, 0xa3, 0x80, 0x9d, 0xe0, 0x51, 0x80,
            0x00, 0x00, 0x01, 0x41, 0x01, 0x42, 0x68, 0xe0, 0x27, 0x78, 0x14, 0x60,
        ];
        let mut decoder = Decoder::default();
        let mut packet = Packet {
            id: u16::MAX,
            reference: 0,
            size: (318, 190),
            key: true,
            payload: &key,
        };
        let surface = decoder.decode(&packet).unwrap().unwrap().pixel_buffer();
        let flags = core_video::pixel_buffer::kCVPixelBufferLock_ReadOnly;
        assert_eq!(surface.lock_base_address(flags), 0);
        let stride = surface.get_bytes_per_row_of_plane(0);
        // Core Video provides a read-only luma plane for the lifetime of the lock.
        let (black, white) = unsafe {
            let plane = surface.get_base_address_of_plane(0).cast::<u8>();
            (*plane.add(48 * stride + 16), *plane.add(144 * stride + 16))
        };
        assert_eq!(surface.unlock_base_address(flags), 0);
        assert!(black <= 4 && white >= 251, "luma range {black}–{white}");

        let mut predicted = Packet {
            id: 0,
            reference: u16::MAX,
            size: packet.size,
            key: false,
            payload: &delta,
        };
        assert!(decoder.decode(&predicted).unwrap().is_some());
        predicted.id = 2;
        predicted.reference = 1;
        assert!(decoder.decode(&predicted).unwrap().is_none());
        predicted.reference = 0;
        assert!(decoder.decode(&predicted).unwrap().is_none());
        packet.id = 3;
        assert!(decoder.decode(&packet).unwrap().is_some());
    }

    #[test]
    fn extracts_parameter_sets_and_reframes_mixed_annex_b_start_codes() {
        let bytes = [0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 1, 0x65, 3];
        let frame = AccessUnit::parse(&bytes).unwrap();
        assert_eq!(frame.sps, Some(&[0x67, 1][..]));
        assert_eq!(frame.pps, Some(&[0x68, 2][..]));
        assert_eq!(frame.avcc, [0, 0, 0, 2, 0x65, 3]);
        assert!(AccessUnit::parse(&[0, 0, 1]).is_none());
        assert!(AccessUnit::parse(&[0x65, 3]).is_none());
    }
}
