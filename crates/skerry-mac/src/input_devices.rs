//! Input devices read from plain CoreAudio properties (`ARCH-038`, M-QA-1).
//!
//! Only `AudioObjectGetPropertyData` on the system object and on each device: the device
//! list, the input stream count, the name, the UID and the default input. Nothing opens an
//! IO proc or an AudioUnit, so listing never raises the macOS microphone (TCC) request.
//! cpal's `input_devices()` must not be used for this: it builds an input AUHAL per device
//! to check `supports_input`, which asks TCC and blocks until the person answers.

use objc2_core_audio::{
    kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectSystemObject, AudioObjectGetPropertyData,
    AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
};
use objc2_core_foundation::{CFRetained, CFString};
use std::ffi::c_void;
use std::ptr::{null, NonNull};

/// The connected inputs as `(raw UID, name)`, and the default input's UID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputList {
    pub devices: Vec<(String, Option<String>)>,
    pub default_uid: Option<String>,
}

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn data_size(object: AudioObjectID, address: AudioObjectPropertyAddress) -> Option<u32> {
    let mut size = 0u32;
    // SAFETY: valid address and out pointer; no qualifier.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
        )
    };
    (status == 0).then_some(size)
}

fn device_ids() -> Option<Vec<AudioObjectID>> {
    let devices = address(
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
    );
    let size = data_size(SYSTEM, devices)?;
    let mut ids = vec![0 as AudioObjectID; size as usize / size_of::<AudioObjectID>()];
    if ids.is_empty() {
        return Some(ids);
    }
    let mut size = (ids.len() * size_of::<AudioObjectID>()) as u32;
    // SAFETY: `ids` holds `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM,
            NonNull::from(&devices),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new(ids.as_mut_ptr().cast::<c_void>())?,
        )
    };
    if status != 0 {
        return None;
    }
    ids.truncate(size as usize / size_of::<AudioObjectID>());
    Some(ids)
}

fn has_input(device: AudioObjectID) -> bool {
    data_size(
        device,
        address(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput),
    )
    .is_some_and(|size| size > 0)
}

fn string(device: AudioObjectID, selector: u32) -> Option<String> {
    let mut value: *const CFString = null();
    let mut size = size_of::<*const CFString>() as u32;
    let address = address(selector, kAudioObjectPropertyScopeGlobal);
    // SAFETY: the property is a CFStringRef returned at +1, written into `value`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    if status != 0 {
        return None;
    }
    // SAFETY: a +1 CFString owned from here on.
    let value = unsafe { CFRetained::from_raw(NonNull::new(value.cast_mut())?) };
    Some(value.to_string())
}

fn default_input() -> Option<AudioObjectID> {
    let mut device: AudioObjectID = 0;
    let mut size = size_of::<AudioObjectID>() as u32;
    let address = address(
        kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal,
    );
    // SAFETY: an AudioObjectID out value of `size` bytes.
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast(),
        )
    };
    (status == 0 && device != 0).then_some(device)
}

/// Lists the inputs; `None` when the device list can't be read.
pub fn list_inputs() -> Option<InputList> {
    let devices = device_ids()?
        .into_iter()
        .filter(|device| has_input(*device))
        .filter_map(|device| {
            let uid = string(device, kAudioDevicePropertyDeviceUID)?;
            Some((uid, string(device, kAudioObjectPropertyName)))
        })
        .collect();
    let default_uid =
        default_input().and_then(|device| string(device, kAudioDevicePropertyDeviceUID));
    Some(InputList {
        devices,
        default_uid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_reads_properties_without_asking_for_the_microphone() {
        let before = crate::permissions::authorization(crate::capture::Subject::Microphone);
        let list = list_inputs().expect("the device list reads");
        // Every listed default is one of the listed devices.
        if let Some(default) = &list.default_uid {
            assert!(
                list.devices.iter().any(|(uid, _)| uid == default),
                "{list:?}"
            );
        }
        // Listing changed nothing about the permission (it would block or flip otherwise).
        let after = crate::permissions::authorization(crate::capture::Subject::Microphone);
        assert_eq!(before, after);
    }
}
