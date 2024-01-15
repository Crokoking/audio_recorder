use std::env;
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

    let mut recorder_builder = PvRecorderBuilder::new(512);

    let library_path = determine_library_path();

    recorder_builder.library_path(&library_path);

    let audio_devices = recorder_builder.get_available_devices().expect("Failed to get available devices");

    if matches.get_flag("list") {
        for (index, audio_device) in audio_devices.iter().enumerate() {
            println!("Device {:?}: {}", index, audio_device);
        }
        return;
    }

    let device_id_option: Option<&i32> = matches.get_one::<i32>("device");

    if let Some(id) = device_id_option {
        println!("Using device {}", audio_devices.get(*id as usize).expect("Invalid device index"));
        recorder_builder.device_index(*id);
    }

    ctrlc::set_handler(move || {
        IS_RUNNING.store(false, Ordering::SeqCst);
        println!("Ctrl-C received!");
    }).expect("Error setting Ctrl-C handler");

    let recorder = recorder_builder.init().expect("Failed to init recorder");
    println!("Starting recorder");
    recorder.start().expect("Failed to start recorder");

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: recorder.sample_rate() as u32,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut wav_writer;
    if let Some(output_path) = matches.get_one::<PathBuf>("output") {
        let file = File::options()
            .write(true)
            .create(true)
            .open(output_path)
            .expect("Failed to open output file");
        wav_writer = WavWriter::new(file, spec).expect("Failed to create WavWriter")
    } else {
        panic!("No output specified")
    }

    while recorder.is_recording() {
        if !IS_RUNNING.load(Ordering::SeqCst) {
            recorder.stop().map_err(|e| eprintln!("Failed to stop recorder: {}", e)).unwrap();
            break;
        }
        let frame_option = recorder.read();
        if frame_option.is_none() {
            eprintln!("Failed to read frame");
            std::process::exit(2);
        }
        let frame = frame_option.unwrap();
        for sample in &frame {
            wav_writer.write_sample(*sample).expect("Failed to write sample");
        }
        wav_writer.flush().expect("Failed to flush wav writer")
    }

    wav_writer.flush().map_err(|e| eprintln!("Failed to flush wav writer: {}", e)).unwrap();
    wav_writer.finalize().map_err(|e| eprintln!("Failed to finalize wav writer: {}", e)).unwrap();
    println!("Done");
}

fn determine_library_path() -> PathBuf {
    let current_exe_path = determine_current_executable();
    let current_exe_directory = current_exe_path.parent().expect("Failed to get current executable's directory");

    // Set the library path based on the OS
    let library_filename = if cfg!(target_os = "windows") {
        "libpv_recorder.dll"
    } else if cfg!(target_os = "macos") {
        "libpv_recorder.dylib"
    } else {
        "libpv_recorder.so"
    };
    let library_path = current_exe_directory.join(library_filename);
    library_path
}

fn determine_current_executable() -> PathBuf {
    let mut current_exe_path = env::current_exe().expect("Failed to get current executable's path");
    let mut counter = 0;
    while current_exe_path.is_symlink() {
        current_exe_path = current_exe_path.read_link().expect("Failed to get current executable's path");
        counter += 1;
        if counter > 10 {
            panic!("Too many symlinks when looking for current executable's path");
        }
    }
    current_exe_path
}
