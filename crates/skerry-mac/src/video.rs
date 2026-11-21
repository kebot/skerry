use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType,
};
use objc2_core_media::{
    kCMBlockBufferAssureMemoryNowFlag,
    kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms, kCMVideoCodecType_AV1,
    kCMVideoCodecType_H264, kCMVideoCodecType_HEVC, CMBlockBuffer, CMFormatDescription,
    CMSampleBuffer, CMTime, CMVideoFormatDescriptionCreate,
    CMVideoFormatDescriptionCreateFromH264ParameterSets,
    CMVideoFormatDescriptionCreateFromHEVCParameterSets, CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::{
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange, CVBuffer,
    CVImageBuffer, CVPixelBufferGetHeight, CVPixelBufferGetIOSurface,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    Jpeg,
    #[default]
    H264,
    Hevc,
    Av1,
}

impl Codec {
    pub const ALL: [Self; 4] = [Self::Jpeg, Self::H264, Self::Hevc, Self::Av1];

    pub fn wire(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::H264 => "h264enc",
            Self::Hevc => "h265enc",
            Self::Av1 => "av1enc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::H264 => "H.264",
            Self::Hevc => "H.265 (HEVC)",
            Self::Av1 => "AV1",
        }
    }

    pub fn decoder_label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::H264 => "H.264 · VideoToolbox",
            Self::Hevc => "H.265 · VideoToolbox",
            Self::Av1 => "AV1 · VideoToolbox",
        }
    }

    pub fn supported(self) -> bool {
        let code = match self {
            Self::Jpeg => return true,
            Self::H264 => kCMVideoCodecType_H264,
            Self::Hevc => kCMVideoCodecType_HEVC,
            Self::Av1 => kCMVideoCodecType_AV1,
        };
        // The query takes a constant Core Media codec identifier.
        unsafe { VTIsHardwareDecodeSupported(code) }
    }
}

pub struct Packet<'a> {
    pub codec: Codec,
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
        let codec = match kind >> 4 {
            0 | 1 => Codec::H264,
            4 => Codec::Av1,
            5 => Codec::Hevc,
            _ => return None,
        };
        if payload.is_empty() {
            return None;
        }
        Some(Self {
            codec,
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
    parameters: [Option<&'a [u8]>; 3],
    avcc: Vec<u8>,
}

impl<'a> AccessUnit<'a> {
    fn parse(codec: Codec, bytes: &'a [u8]) -> Option<Self> {
        let mut unit = Self {
            parameters: [None; 3],
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
            let header = *nalu.first()?;
            let parameter = match codec {
                Codec::H264 => match header & 0x1f {
                    7 => Some(0),
                    8 => Some(1),
                    _ => None,
                },
                Codec::Hevc => match (header >> 1) & 0x3f {
                    32..=34 => Some(usize::from(((header >> 1) & 0x3f) - 32)),
                    _ => None,
                },
                _ => return None,
            };
            if let Some(index) = parameter {
                unit.parameters[index] = Some(nalu);
            } else {
                unit.avcc
                    .extend_from_slice(&u32::try_from(nalu.len()).ok()?.to_be_bytes());
                unit.avcc.extend_from_slice(nalu);
            }
            let Some(next) = next else { break };
            remaining = &remaining[next + 3..];
        }
        Some(unit)
    }
}

#[derive(Clone)]
pub struct Surface(CFRetained<CVBuffer>);

// VideoToolbox has finished writing before publication. The retained buffer is
// only shared for Metal reads, and Core Foundation reference counting is atomic.
unsafe impl Send for Surface {}
unsafe impl Sync for Surface {}

impl Surface {
    pub fn pixel_buffer(&self) -> CFRetained<CVBuffer> {
        self.0.clone()
    }
}

struct Session {
    session: CFRetained<VTDecompressionSession>,
    format: CFRetained<CMFormatDescription>,
    codec: Codec,
    size: (u16, u16),
    config: Vec<Vec<u8>>,
}

// Each session is exclusively owned by one task. VideoToolbox sessions have no
// thread affinity, and this decoder uses synchronous submit and output.
unsafe impl Send for Session {}

impl Session {
    fn new(codec: Codec, config: &[&[u8]], size: (u16, u16)) -> Result<Self, &'static str> {
        let format = if codec == Codec::Av1 {
            av1_format(config[0], size)
        } else {
            nal_format(codec, config)
        }
        .ok_or("Invalid video codec configuration.")?;
        // Verify the actual coded size before asking VideoToolbox to allocate surfaces.
        let coded = unsafe { CMVideoFormatDescriptionGetDimensions(&format) };
        if (coded.width, coded.height) != (i32::from(size.0), i32::from(size.1)) {
            return Err("Video configuration disagrees with the display size.");
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
            .ok_or("Couldn’t start the hardware video decoder.")?;
        Ok(Self {
            session,
            format,
            codec,
            size,
            config: config.iter().map(|part| part.to_vec()).collect(),
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
            unsafe { take_created(status, block) }.ok_or("Couldn’t allocate the video sample.")?;
        if unsafe {
            CMBlockBuffer::replace_data_bytes(NonNull::from(&bytes[0]).cast(), &block, 0, len)
        } != 0
        {
            return Err("Couldn’t fill the video sample.");
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
            unsafe { take_created(status, sample) }.ok_or("Couldn’t create the video sample.")?;
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
            return Err("Hardware video decode failed.");
        }
        output.ok_or("The video decoder returned no renderable surface.")
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
    if CVPixelBufferGetPixelFormatType(image) == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange {
        let buffer = CFRetained::from(image);
        unsafe {
            *output.cast::<Option<Surface>>() = Some(Surface(buffer));
        }
    }
}

fn nal_format(codec: Codec, config: &[&[u8]]) -> Option<CFRetained<CMFormatDescription>> {
    let pointers = config
        .iter()
        .map(|part| part.first().map(NonNull::from))
        .collect::<Option<Vec<_>>>()?;
    let lengths: Vec<_> = config.iter().map(|part| part.len()).collect();
    let mut format = null();
    // The arrays and each parameter set remain live for the call.
    let status = unsafe {
        let pointers = NonNull::from(pointers.first()?);
        let lengths = NonNull::from(lengths.first()?);
        let out = NonNull::from(&mut format);
        if codec == Codec::H264 {
            CMVideoFormatDescriptionCreateFromH264ParameterSets(
                None,
                config.len(),
                pointers,
                lengths,
                4,
                out,
            )
        } else {
            CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                None,
                config.len(),
                pointers,
                lengths,
                4,
                None,
                out,
            )
        }
    };
    unsafe { take_created(status, format.cast_mut()) }
}

// Selkies sends low-overhead AV1 OBUs. The first sequence header in a keyframe
// supplies the configuration OBU for the av1C atom consumed by VideoToolbox.
fn av1_sequence(mut data: &[u8]) -> Option<(&[u8], &[u8])> {
    while let Some(&header) = data.first() {
        if header & 0x83 != 2 {
            return None;
        }
        let size_start = 1 + usize::from(header & 4 != 0);
        let size_bytes = data.get(size_start..)?;
        let last = size_bytes.iter().take(4).position(|byte| *byte < 128)?;
        let size = size_bytes[..=last]
            .iter()
            .enumerate()
            .fold(0usize, |size, (index, byte)| {
                size | (usize::from(byte & 127) << (index * 7))
            });
        let payload_start = size_start + last + 1;
        let end = payload_start + size;
        let obu = data.get(..end)?;
        if (header >> 3) & 15 == 1 {
            return Some((obu, &obu[payload_start..]));
        }
        data = &data[end..];
    }
    None
}

fn av1_format(obu: &[u8], size: (u16, u16)) -> Option<CFRetained<CMFormatDescription>> {
    let (_, payload) = av1_sequence(obu)?;
    let mut bits = payload
        .iter()
        .flat_map(|byte| (0..8).rev().map(move |shift| (byte >> shift) & 1));
    fn read_bits(bits: &mut impl Iterator<Item = u8>, count: usize) -> Option<u8> {
        (0..count).try_fold(0u8, |value, _| Some((value << 1) | bits.next()?))
    }
    // Support the normal Main 8-bit 4:2:0 streams negotiated below.
    if read_bits(&mut bits, 3)? != 0 {
        return None;
    }
    bits.next()?; // still_picture
    let reduced = bits.next()? != 0;
    if !reduced {
        if bits.next()? != 0 {
            bits.nth(63)?; // timing information
            if bits.next()? != 0 {
                let leading = (0..32).find(|_| bits.next() != Some(0))?;
                for _ in 0..leading {
                    bits.next()?;
                }
            }
            if bits.next()? != 0 {
                bits.nth(46)?;
            } // decoder model info
        }
        bits.nth(17)?; // display delay flag, operating point count, first operating point ID
    }
    let level = read_bits(&mut bits, 5)?;
    let tier = if !reduced && level > 7 {
        bits.next()?
    } else {
        0
    };
    let mut av1c = vec![0x81, level, (tier << 7) | 0x0c, 0];
    av1c.extend_from_slice(obu);
    let atom = CFData::from_bytes(&av1c);
    let atom_name = CFString::from_str("av1C");
    let atoms = CFDictionary::from_slices(&[&*atom_name], &[&*atom]);
    let mut format = null();
    // Core Media retains the extension dictionary when creating the format description.
    let status = unsafe {
        let extensions = CFDictionary::from_slices(
            &[kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms],
            &[&*atoms],
        );
        CMVideoFormatDescriptionCreate(
            None,
            kCMVideoCodecType_AV1,
            size.0.into(),
            size.1.into(),
            Some(extensions.as_opaque()),
            NonNull::from(&mut format),
        )
    };
    unsafe { take_created(status, format.cast_mut()) }
}

/// Whether a non-key packet continues from the last decoded frame (`last`).
fn follows(last: Option<u16>, id: u16, reference: u16) -> bool {
    if reference == id {
        last.is_some()
    } else {
        last == Some(reference)
    }
}

#[derive(Default)]
pub struct Decoder {
    session: Option<Session>,
    last: Option<u16>,
}

impl Decoder {
    pub fn decode(&mut self, packet: &Packet<'_>) -> Result<Option<Surface>, &'static str> {
        let matches = self
            .session
            .as_ref()
            .is_some_and(|session| session.codec == packet.codec && session.size == packet.size);
        // H.264 packets name the frame they predict from. Selkies 2.3.0's H.265 and AV1
        // encoders send `reference == id` instead (no chain information), so for those the
        // previous frame must simply have decoded; the WebSocket is ordered and lossless, and a
        // failed decode clears `last` and waits for the next keyframe either way.
        if !packet.key && (!matches || !follows(self.last, packet.id, packet.reference)) {
            self.last = None;
            return Ok(None);
        }
        self.last = None;
        let framed;
        let (sample, config) = if packet.codec == Codec::Av1 {
            let config = if packet.key {
                vec![
                    av1_sequence(packet.payload)
                        .ok_or("Missing AV1 sequence header.")?
                        .0,
                ]
            } else {
                Vec::new()
            };
            (packet.payload, config)
        } else {
            framed = AccessUnit::parse(packet.codec, packet.payload)
                .ok_or("Invalid Annex B access unit.")?;
            let config = if packet.key {
                let count = if packet.codec == Codec::Hevc { 3 } else { 2 };
                framed.parameters[..count]
                    .iter()
                    .copied()
                    .collect::<Option<Vec<_>>>()
                    .ok_or("Missing video parameter sets.")?
            } else {
                Vec::new()
            };
            (framed.avcc.as_slice(), config)
        };
        if packet.key
            && (!matches
                || !self
                    .session
                    .as_ref()
                    .is_some_and(|session| session.config == config))
        {
            self.session = Some(Session::new(packet.codec, &config, packet.size)?);
        }
        if sample.is_empty() {
            return Err("The packet contains no picture data.");
        }
        let session = self
            .session
            .as_ref()
            .ok_or("The video decoder needs a keyframe.")?;
        let surface = session.decode(sample)?;
        if (
            CVPixelBufferGetWidth(&surface.0),
            CVPixelBufferGetHeight(&surface.0),
        ) != (usize::from(packet.size.0), usize::from(packet.size.1))
        {
            return Err("The decoded picture size doesn’t match the packet.");
        }
        self.last = Some(packet.id);
        Ok(Some(surface))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predicted_frames_follow_chained_or_unchained_references() {
        // H.264: the reference names the previous decoded packet.
        assert!(follows(Some(4), 6, 4));
        assert!(!follows(Some(4), 6, 5));
        // H.265 / AV1 on Selkies 2.3.0: reference == id, so only continuity is required.
        assert!(follows(Some(10), 46, 46));
        assert!(!follows(None, 46, 46));
        assert!(!follows(None, 1, 0));
    }

    #[test]
    fn finds_the_av1_sequence_obu_and_rejects_truncated_lengths() {
        let bytes = [0x12, 0, 0x0a, 2, 0x08, 0, 0x32, 1, 0];
        assert_eq!(av1_sequence(&bytes), Some((&bytes[2..6], &bytes[4..6])));
        assert!(av1_sequence(&[0x0a, 0x80]).is_none());
        assert!(av1_sequence(&[0x0a, 5, 0]).is_none());
    }

    #[test]
    fn distinguishes_hevc_and_av1_wire_packets() {
        let mut bytes = [4, 0x51, 0, 1, 0, 0, 5, 0, 2, 208, 0, 0, 1];
        assert_eq!(Packet::parse(&bytes).unwrap().codec, Codec::Hevc);
        bytes[1] = 0x41;
        assert_eq!(Packet::parse(&bytes).unwrap().codec, Codec::Av1);
    }

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
        if !Codec::H264.supported() {
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
            codec: Codec::H264,
            id: u16::MAX,
            reference: 0,
            size: (318, 190),
            key: true,
            payload: &key,
        };
        let surface = decoder.decode(&packet).unwrap().unwrap().pixel_buffer();
        let flags = objc2_core_video::CVPixelBufferLockFlags::ReadOnly;
        assert_eq!(
            unsafe { objc2_core_video::CVPixelBufferLockBaseAddress(&surface, flags) },
            0
        );
        let stride = objc2_core_video::CVPixelBufferGetBytesPerRowOfPlane(&surface, 0);
        // Core Video provides a read-only luma plane for the lifetime of the lock.
        let (black, white) = unsafe {
            let plane =
                objc2_core_video::CVPixelBufferGetBaseAddressOfPlane(&surface, 0).cast::<u8>();
            (*plane.add(48 * stride + 16), *plane.add(144 * stride + 16))
        };
        assert_eq!(
            unsafe { objc2_core_video::CVPixelBufferUnlockBaseAddress(&surface, flags) },
            0
        );
        assert!(black <= 4 && white >= 251, "luma range {black}–{white}");

        let mut predicted = Packet {
            codec: Codec::H264,
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
        let frame = AccessUnit::parse(Codec::H264, &bytes).unwrap();
        assert_eq!(frame.parameters[0], Some(&[0x67, 1][..]));
        assert_eq!(frame.parameters[1], Some(&[0x68, 2][..]));
        assert_eq!(frame.avcc, [0, 0, 0, 2, 0x65, 3]);
        assert!(AccessUnit::parse(Codec::H264, &[0, 0, 1]).is_none());
        assert!(AccessUnit::parse(Codec::H264, &[0x65, 3]).is_none());
    }
}
