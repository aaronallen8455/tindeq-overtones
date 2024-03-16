use std::sync::{Arc, Mutex};
use std::error::Error;

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

const SERVICE_UUID: Uuid = uuid!("7e4e1701-1ea6-40c9-9dcc-13d34ffead57");
const CONTROL_UUID: Uuid = uuid!("7e4e1703-1ea6-40c9-9dcc-13d34ffead57");
const DATA_UUID: Uuid = uuid!("7e4e1702-1ea6-40c9-9dcc-13d34ffead57");

// opcodes
const START_WEIGHT_MEASUREMENT: u8 = 0x65;
const END_WEIGHT_MEASUREMENT: u8 = 0x66;

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

// Connect to progressor and record weight measurements
pub async fn interaction(running: Arc<Mutex<bool>>, cur_weight: Arc<Mutex<f32>>)
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
    let service = services
        .iter()
        .find(|s| s.uuid == SERVICE_UUID)
        .ok_or("service not found")?;

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
