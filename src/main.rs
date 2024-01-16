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

static LIB_ERROR: i32 = 1;
static USER_ERROR: i32 = 2;
static AUDIO_ERROR: i32 = 3;
static FILE_ERROR: i32 = 4;

fn main() {
    let matches = command!() // requires `cargo` feature
        .arg(Arg::new("device").short('d').long("device").required(false).value_parser(value_parser!(i32)))
        .arg(Arg::new("output").short('o').long("output").required(true).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("lib").long("lib").required(false).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("list").short('l').long("list").required(false).action(ArgAction::SetTrue))
        .get_matches();

    let mut recorder_builder = PvRecorderBuilder::new(512);

    let library_path = match matches.get_one::<PathBuf>("lib") {
        Some(path) => path.clone(),
        None => match determine_library_path() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Failed to determine library path: {}", error);
                std::process::exit(LIB_ERROR);
            }
        }
    };

    println!("Using library {}", library_path.to_string_lossy());

    recorder_builder.library_path(&library_path);

    let audio_devices = match recorder_builder.get_available_devices() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("Failed to get available devices: {}", error);
            std::process::exit(AUDIO_ERROR);
        }
    };

    if matches.get_flag("list") {
        for (index, audio_device) in audio_devices.iter().enumerate() {
            println!("Device {:?}: {}", index, audio_device);
        }
        return;
    }

    if let Some(id) = matches.get_one::<i32>("device") {
        if let Some(device) = audio_devices.get(*id as usize) {
            println!("Using device {}", device);
            recorder_builder.device_index(*id);
        } else  {
            eprintln!("Invalid device index {} specified", id);
            std::process::exit(USER_ERROR);
        }
    }

    ctrlc::set_handler(move || {
        IS_RUNNING.store(false, Ordering::SeqCst);
        println!("Ctrl-C received!");
    }).expect("Error setting Ctrl-C handler");

    let recorder = match recorder_builder.init() {
        Ok(recorder) => recorder,
        Err(error) => {
            eprintln!("Failed to initialize recorder: {}", error);
            std::process::exit(AUDIO_ERROR);
        }
    };
    println!("Starting recorder");
    if let Err(error) = recorder.start() {
        eprintln!("Failed to start recorder: {}", error);
        std::process::exit(AUDIO_ERROR);
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: recorder.sample_rate() as u32,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let mut wav_writer;
    if let Some(output_path) = matches.get_one::<PathBuf>("output") {
        let file_result = File::options()
            .write(true)
            .create(true)
            .open(output_path);
        let file = match file_result {
            Ok(file) => file,
            Err(error) => {
                eprintln!("Failed to open output file: {}", error);
                std::process::exit(FILE_ERROR);
            }
        };
        wav_writer = match WavWriter::new(file, spec) {
            Ok(wav_writer) => wav_writer,
            Err(error) => {
                eprintln!("Failed to create wav writer: {}", error);
                std::process::exit(AUDIO_ERROR);
            }
        }
    } else {
        eprintln!("No output file specified");
        std::process::exit(USER_ERROR);
    }

    while recorder.is_recording() {
        if !IS_RUNNING.load(Ordering::SeqCst) {
            if let Err(error) = recorder.stop() {
                eprintln!("Failed to stop recorder: {}", error);
                std::process::exit(AUDIO_ERROR);
            }
            break;
        }
        match recorder.read() {
            Ok(frame) => {
                for sample in &frame {
                    if let Err(error) = wav_writer.write_sample(*sample) {
                        eprintln!("Failed to write sample: {}", error);
                        std::process::exit(FILE_ERROR);
                    }
                }
                if let Err(error) = wav_writer.flush() {
                    eprintln!("Failed to flush wav writer: {}", error);
                    std::process::exit(FILE_ERROR);
                }
            }
            Err(error) => {
                eprintln!("Failed to read frame: {}", error);
                std::process::exit(AUDIO_ERROR);
            }
        }
    }

    if let Err(error) = wav_writer.flush() {
        eprintln!("Failed to flush wav writer: {}", error);
        std::process::exit(FILE_ERROR);
    }
    if let Err(error) = wav_writer.finalize() {
        eprintln!("Failed to finalize wav writer: {}", error);
        std::process::exit(FILE_ERROR);
    }
    println!("Done");
}

fn determine_library_path() -> Result<PathBuf, String> {
    let current_exe_path = match determine_current_executable() {
        Ok(path) => path,
        Err(error) => {
            return Err(format!("Failed to determine current executable: {}", error));
        }
    };
    let current_exe_directory = match current_exe_path.parent() {
        Some(directory) => directory,
        None => {
            return Err("Failed to determine current executable directory".to_string());
        }
    };

    // Set the library path based on the OS
    let library_filename = if cfg!(target_os = "windows") {
        "libpv_recorder.dll"
    } else if cfg!(target_os = "macos") {
        "libpv_recorder.dylib"
    } else {
        "libpv_recorder.so"
    };
    let library_path = current_exe_directory.join(library_filename);
    return Ok(library_path)
}

fn determine_current_executable() -> Result<PathBuf, String> {
    let mut current_exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            return Err(format!("Failed to determine current executable path: {}", error));
        }
    };
    let mut counter = 0;
    while current_exe_path.is_symlink() {
        current_exe_path = match current_exe_path.read_link() {
            Ok(path) => path,
            Err(error) => {
                return Err(format!("Failed to read symlink {}: {}", current_exe_path.to_string_lossy(), error));
            }
        };
        counter += 1;
        if counter > 10 {
            return Err("Too many symlinks when trying to determine the executable path".to_string());
        }
    }
    Ok(current_exe_path)
}
