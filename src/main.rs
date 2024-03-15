use std::error::Error;
use std::sync::{Arc, Mutex};
use std::io;

use uuid::{uuid, Uuid};
use bluest::{Adapter};
use futures_lite::StreamExt;
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
    interact_progressor(running, cur_weight).await?;

    Ok(())
}

fn mk_stream(cur_weight: Arc<Mutex<f32>>) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let host = cpal::default_host();
    let device = host.default_output_device().expect("failed to find output device");
    let config = device.default_output_config().unwrap();

    match config.sample_format() {
        cpal::SampleFormat::F32 => create_stream(&device, &config.into()),
        sample_format => panic!("Unsupported sample format {sample_format}")
    }
}

fn create_stream(device: &cpal::Device, config: &cpal::StreamConfig)
    -> Result<cpal::Stream, cpal::BuildStreamError>
{
    let sample_rate = config.sample_rate.0 as f32;
    let channels = config.channels as usize;
    let mut sample_clock = 0f32;
    let mut next_value = move || {
        sample_clock = (sample_clock + 1.0) % sample_rate; // why?
        (sample_clock * 440.0 * 2.0 * std::f32::consts::PI / sample_rate).sin()
    };

    println!("{sample_rate}");
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

fn wave_sample(a: f32, t: f32, f: f32, p: f32) -> f32 {
    a * f32::sin(2.0 * std::f32::consts::PI * f * t + p)
}

async fn interact_progressor(running: Arc<Mutex<bool>>, cur_weight: Arc<Mutex<f32>>)
    -> Result<(), Box<dyn Error>> {
    let adapter = Adapter::default().await.ok_or("Bluetooth adapter not found")?;
    adapter.wait_available().await?;

    let discovered_device = {
        println!("starting scan");
        let mut scan = adapter.scan(&[SERVICE_UUID]).await?;
        println!("scan started");
        scan.next().await.ok_or("scan_terminated")?
    };

    println!("{:?} {:?}", discovered_device.rssi, discovered_device.adv_data);
    let device = discovered_device.device;

    adapter.connect_device(&device).await?;
    println!("connected");

    let service = match device
        .discover_services_with_uuid(SERVICE_UUID)
        .await?
        .first()
        {
            Some(service) => service.clone(),
            None => return Err("service not found".into()),
        };

    let characteristics = service.characteristics().await?;
    let control_characteristic = characteristics
        .iter()
        .find(|x| x.uuid() == CONTROL_UUID)
        .ok_or("control characteristic not found")?;
    let data_characteristic = characteristics
        .iter()
        .find(|x| x.uuid() == DATA_UUID)
        .ok_or("data characteristic not found")?;

    control_characteristic.write(&[START_WEIGHT_MEASUREMENT, 0]).await?;

    let mut notifications = data_characteristic.notify().await?;

    while let Some(Ok(x)) = notifications.next().await
    {
        if !(*running.lock().unwrap()) { break }
        match parse_response(x) {
            Some(Response::WeightMeasurement(w, _)) =>
                *cur_weight.lock().unwrap() = w,
            _ => (),
        }
    }

    control_characteristic.write(&[END_WEIGHT_MEASUREMENT, 0]).await?;

    adapter.disconnect_device(&device).await?;
    println!("disconnected");

    Ok(())
}
