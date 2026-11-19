use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus_decoder::OpusDecoder;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const QUEUE_LIMIT: usize = RATE as usize * CHANNELS / 8;

pub struct Playback {
    _output: cpal::Stream,
    queue: Arc<Mutex<VecDeque<f32>>>,
    decoder: OpusDecoder,
    pcm: Vec<f32>,
}

impl Playback {
    pub fn new() -> Option<Self> {
        let device = cpal::default_host().default_output_device()?;
        let config = device
            .supported_output_configs()
            .ok()?
            .find(|cfg| {
                cfg.channels() == CHANNELS as u16
                    && cfg.sample_format() == cpal::SampleFormat::F32
                    && (cfg.min_sample_rate()..=cfg.max_sample_rate()).contains(&RATE)
            })?
            .with_sample_rate(RATE)
            .config();
        let queue = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(QUEUE_LIMIT)));
        let output_queue = queue.clone();
        let output = device
            .build_output_stream(
                &config,
                move |out: &mut [f32], _| {
                    if let Ok(mut queue) = output_queue.try_lock() {
                        for sample in out {
                            *sample = queue.pop_front().unwrap_or(0.0);
                        }
                    } else {
                        out.fill(0.0);
                    }
                },
                |_| {},
                None,
            )
            .ok()?;
        let decoder = OpusDecoder::new(RATE, CHANNELS).ok()?;
        output.play().ok()?;
        Some(Self {
            _output: output,
            queue,
            decoder,
            pcm: vec![0.0; 5760 * CHANNELS],
        })
    }

    pub fn packet(&mut self, packet: &[u8]) {
        // We advertise audioRedundancy=false, so only the plain Opus payload is used.
        if packet.len() <= 2 || packet[1] & 0x7f != 0 {
            return;
        }
        let Ok(samples) = self
            .decoder
            .decode_float(&packet[2..], &mut self.pcm, false)
        else {
            return;
        };
        if let Ok(mut queue) = self.queue.lock() {
            let pcm = &self.pcm[..samples * CHANNELS];
            let dropped = (queue.len() + pcm.len())
                .saturating_sub(QUEUE_LIMIT)
                .min(queue.len());
            queue.drain(..dropped);
            queue.extend(pcm);
        }
    }

    pub fn clear(&mut self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }
}
