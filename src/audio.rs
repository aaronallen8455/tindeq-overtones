use std::sync::{Arc, Mutex};

use cpal::{
    traits::{DeviceTrait, HostTrait}
};

pub fn mk_stream(cur_weight: Arc<Mutex<f32>>) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let host = cpal::default_host();
    let device = host.default_output_device().expect("failed to find output device");
    let config = device.default_output_config().unwrap();

    match config.sample_format() {
        cpal::SampleFormat::F32 => create_stream(cur_weight, &device, &config.into()),
        sample_format => panic!("Unsupported sample format {sample_format}")
    }
}

// Maps a weight value to a frequency in the overtone series of A110
fn weight_to_freq(weight: f32) -> f32 {
    110.0 * (weight.trunc() + 1.0)
}

fn create_stream(cur_weight: Arc<Mutex<f32>>, device: &cpal::Device, config: &cpal::StreamConfig)
    -> Result<cpal::Stream, cpal::BuildStreamError>
{
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;
    let mut sample_clock = 0f32;
    let mut phase = 0.0;
    let mut phase_offset = 0.0;
    let mut prev_freq = 0.0;
    let mut next_value = move || {
        sample_clock = (sample_clock + 1.0) % sample_rate;
        let freq = weight_to_freq(*cur_weight.lock().unwrap());
        if freq != prev_freq {
            prev_freq = freq;
            sample_clock = 1.0;
            phase_offset = phase % (2.0 * std::f32::consts::PI);
        }
        phase = sample_clock * freq * 2.0 * std::f32::consts::PI / sample_rate
            + phase_offset;
        phase.sin()
    };

    device.build_output_stream(
        config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            write_data(data, channels, &mut next_value)
        },
        |err| eprintln!("an error occurred on stream: {}", err),
        None
        )
}

fn write_data(output: &mut [f32], channels: usize, next_sample: &mut dyn FnMut() -> f32) {
    for frame in output.chunks_mut(channels) {
        for sample in frame.iter_mut() {
            *sample = next_sample()
        }
    }
}

