use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Arg, ArgAction, command, value_parser};
use ctrlc;
use hound::SampleFormat;
use hound::WavWriter;
use pv_recorder::PvRecorderBuilder;

static IS_RUNNING: AtomicBool = AtomicBool::new(true);

fn main() {
    let matches = command!() // requires `cargo` feature
        .arg(Arg::new("device").short('d').long("device").required(false).value_parser(value_parser!(i32)))
        .arg(Arg::new("output").short('o').long("output").required(true).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("list").short('l').long("list").required(false).action(ArgAction::SetTrue))
        .get_matches();

    if matches.get_flag("list") {
        list_devices();
        std::process::exit(0);
    }

    let device_id: Option<&i32> = matches.get_one::<i32> ("device");
    let mut recorder_builder = PvRecorderBuilder::new(512);
    if let Some(id) = device_id {
        recorder_builder.device_index(*id);
    }

    ctrlc::set_handler(move || {
        IS_RUNNING.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let recorder = recorder_builder.init().expect("Failed to init recorder");
    recorder.start().expect("Failed to start recorder");

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: recorder.sample_rate() as u32,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut wav_writer ;
    if let Some(output_path) = matches.get_one::<PathBuf>("output") {
        let file = File::options()
            .write(true)
            .create(true)
            .open(output_path)
            .expect("Failed to open output file");
        wav_writer= WavWriter::new(file, spec).expect("Failed to create WavWriter")
    } else {
        panic!("No output specified")
    }

    while recorder.is_recording() {
        if !IS_RUNNING.load(Ordering::SeqCst) {
            recorder.stop().map_err(|e| println!("Failed to stop recorder: {}", e)).unwrap();
            break;
        }
        let frame = recorder.read().expect("Failed to read frame");
        for sample in &frame {
            wav_writer.write_sample(*sample).expect("Failed to write sample");
        }
    }

    wav_writer.flush().map_err(|e| println!("Failed to flush wav writer: {}", e)).unwrap();
    wav_writer.finalize().map_err(|e| println!("Failed to finalize wav writer: {}", e)).unwrap();
}

fn list_devices() {
    let audio_devices: Vec<String> = PvRecorderBuilder::default().get_available_devices().expect("Failed to get available devices");
    for (index, audio_device) in audio_devices.iter().enumerate() {
        println!("Device {:?}: {}", index, audio_device);
    }
}
