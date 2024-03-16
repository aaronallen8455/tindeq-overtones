use std::error::Error;
use std::sync::{Arc, Mutex};
use std::io;

use futures_lite::stream::StreamExt;
use uuid::{uuid, Uuid};
use btleplug::{
    platform::Manager,
    api::{
        Manager as _,
        Central,
        ScanFilter,
        CentralEvent,
        Peripheral,
        WriteType,
    }
};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait}
};

const SERVICE_UUID: Uuid = uuid!("7e4e1701-1ea6-40c9-9dcc-13d34ffead57");
const CONTROL_UUID: Uuid = uuid!("7e4e1703-1ea6-40c9-9dcc-13d34ffead57");
const DATA_UUID: Uuid = uuid!("7e4e1702-1ea6-40c9-9dcc-13d34ffead57");

// opcodes
const START_WEIGHT_MEASUREMENT: u8 = 0x65;
const END_WEIGHT_MEASUREMENT: u8 = 0x66;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let running = Arc::new(Mutex::new(true));
    let cur_weight = Arc::new(Mutex::new(0.0));
    let r2 = running.clone();

    let stream = mk_stream(cur_weight.clone())?;

    // Stop on user input
    tokio::spawn(async move {
        let mut i = String::new();
        io::stdin().read_line(&mut i).expect("what?");
        let mut running = r2.lock().unwrap();
        *running = false;
    });

    stream.play()?;
    progressor_interaction(running, cur_weight).await?;

    Ok(())
}

fn mk_stream(cur_weight: Arc<Mutex<f32>>) -> Result<cpal::Stream, cpal::BuildStreamError> {
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
    let mut next_value = move || {
        sample_clock = (sample_clock + 1.0) % sample_rate;
        let freq = weight_to_freq(*cur_weight.lock().unwrap());
        (sample_clock * freq * 2.0 * std::f32::consts::PI / sample_rate).sin()
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

#[derive(Debug)]
enum Response {
    WeightMeasurement(f32, u32),
    SampleBatteryVoltage(u32),
    LowPowerWarning,
}

fn parse_response(i: Vec<u8>) -> Option<Response> {
    let code = i.get(0)?;
    match code {
        0 => Some(Response::SampleBatteryVoltage(u32::from_le_bytes(i[2..6].try_into().ok()?))),
        1 => Some(Response::WeightMeasurement(
                    f32::from_le_bytes(i[2..6].try_into().ok()?),
                    u32::from_le_bytes(i[6..10].try_into().ok()?)
                    )
                ),
        4 => Some(Response::LowPowerWarning),
        _ => None
    }
}

async fn progressor_interaction(running: Arc<Mutex<bool>>, cur_weight: Arc<Mutex<f32>>)
    -> Result<(), Box<dyn Error>> {
    let manager = Manager::new().await?;
    // Get the first bluetooth adapter
    let adapters = manager.adapters().await.expect("unable to fetch adapters");
    let central = adapters.get(0).expect("no adapters");

    central.start_scan(ScanFilter { services : vec![SERVICE_UUID] }).await?;

    let events = central.events().await?;
    let device_id = events.filter_map
        (|x| match x {
            CentralEvent::DeviceDiscovered(id) => Some(id),
            _ => None,
        }
        ).next().await.ok_or("device not found")?;

    let device = central.peripheral(&device_id).await?;

    device.connect().await?;
    println!("connected");

    device.discover_services().await?;

    let services = device.services();
    let service = services.iter().find(
        |s| s.uuid == SERVICE_UUID
        ).ok_or("service not found")?;

    let control_characteristic = service.characteristics
        .iter()
        .find(|x| x.uuid == CONTROL_UUID)
        .ok_or("control characteristic not found")?;
    let data_characteristic = service.characteristics
        .iter()
        .find(|x| x.uuid == DATA_UUID)
        .ok_or("data characteristic not found")?;

    device.subscribe(data_characteristic).await?;
    device.write(
        control_characteristic,
        &[START_WEIGHT_MEASUREMENT, 0],
        WriteType::WithResponse
    ).await?;

    let mut notifications = device.notifications().await?;

    while let Some(x) = notifications.next().await
    {
        if !(*running.lock().unwrap()) { break }
        match parse_response(x.value) {
            Some(Response::WeightMeasurement(w, _)) =>
                *cur_weight.lock().unwrap() = w,
            _ => (),
        }
    }

    device.write(
        control_characteristic,
        &[END_WEIGHT_MEASUREMENT, 0],
        WriteType::WithResponse
    ).await?;

    device.disconnect().await?;
    println!("disconnected");

    Ok(())
}
