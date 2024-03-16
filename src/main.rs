use std::error::Error;
use std::sync::{Arc, Mutex};
use std::io;

use cpal::traits::StreamTrait;

mod audio;
mod progressor;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let running = Arc::new(Mutex::new(true));
    let cur_weight = Arc::new(Mutex::new(0.0));
    let r2 = running.clone();

    let stream = audio::mk_stream(cur_weight.clone())?;

    // Stop on user input
    tokio::spawn(async move {
        let mut i = String::new();
        io::stdin().read_line(&mut i).expect("what?");
        let mut running = r2.lock().unwrap();
        *running = false;
    });

    stream.play()?;
    progressor::interaction(running, cur_weight).await?;

    Ok(())
}
