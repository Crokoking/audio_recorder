use std::env;
use std::fs::File;
use std::io::{stdout, StdoutLock, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Arg, ArgAction, ArgMatches, command, value_parser};
use ctrlc;
use hound::SampleFormat;
use hound::WavWriter;
use pv_recorder::PvRecorderBuilder;

static IS_RUNNING: AtomicBool = AtomicBool::new(true);

static LIB_ERROR: i32 = 1;
static USER_ERROR: i32 = 2;
static AUDIO_ERROR: i32 = 3;
static FILE_ERROR: i32 = 4;


/** Smoothing factor for the moving average */
const ALPHA: f32 = 0.1;

const SILENCE_FACTOR: f32 = 0.9;

const BYTE_CONFIG: u8 = 1u8;
const BYTE_FRAME: u8 = 2u8;
const BYTE_CURRENT_VERSION: u8 = 1u8;

/*
Stream format:
<stream header> <frame bytes> <frame bytes> <frame bytes> ...
<stream header> = <version byte = 1> <config byte = 1> <sample rate bytes LE = u32>
<frame bytes> = <frame init byte = 2> <silence detection byte = 0(noise)/1(silence)> <frame length bytes LE = u32>
    <sample bytes LE = i16> <sample bytes LE = i16> <sample bytes LE = i16> ...

*/

fn main() {
    let matches = command!() // requires `cargo` feature
        .arg(Arg::new("device").short('d').long("device").required(false).value_parser(value_parser!(i32)))
        .arg(Arg::new("output").short('o').long("output").required(false).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("stream").long("stream").required(false).action(ArgAction::SetTrue).help("Stream audio to stdout instead of writing to a file"))
        .arg(Arg::new("lib").long("lib").required(false).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("list").short('l').long("list").required(false).action(ArgAction::SetTrue))
        .arg(Arg::new("stop-silence").short('s').long("stop-silence").required(false).value_parser(value_parser!(u64)).help("Stop recording after this many milliseconds of silence"))
        .get_matches();

    if matches.get_flag("stream") && matches.get_one::<PathBuf>("output").is_some() {
        eprintln!("Cannot specify both --stream and --output");
        std::process::exit(USER_ERROR);
    }

    if !matches.get_flag("stream") && matches.get_one::<PathBuf>("output").is_none() {
        eprintln!("Must specify either --stream or --output");
        std::process::exit(USER_ERROR);
    }

    let mut recorder_builder = create_recorder_builder(&matches);

    let audio_devices = match recorder_builder.get_available_devices() {
        Ok(devices) => devices,
        Err(error) => {
            eprintln!("Failed to get available devices: {}", error);
            std::process::exit(AUDIO_ERROR);
        }
    };

    if matches.get_flag("list") {
        for (index, audio_device) in audio_devices.iter().enumerate() {
            eprintln!("Device {:?}: {}", index, audio_device);
        }
        return;
    }

    if let Some(id) = matches.get_one::<i32>("device") {
        if let Some(device) = audio_devices.get(*id as usize) {
            eprintln!("Using device {}", device);
            recorder_builder.device_index(*id);
        } else {
            eprintln!("Invalid device index {} specified", id);
            std::process::exit(USER_ERROR);
        }
    }

    let silence_threshold_ms = matches.get_one::<u64>("stop-silence").copied().unwrap_or(0);

    if let Err(error) = ctrlc::set_handler(move || {
        IS_RUNNING.store(false, Ordering::SeqCst);
        eprintln!("Ctrl-C received!");
    }) {
        eprintln!("Failed to set Ctrl-C handler: {}", error);
        std::process::exit(FILE_ERROR);
    }

    let recorder = match recorder_builder.init() {
        Ok(recorder) => recorder,
        Err(error) => {
            eprintln!("Failed to initialize recorder: {}", error);
            std::process::exit(AUDIO_ERROR);
        }
    };

    eprintln!("Starting recorder");
    let start_result = recorder.start();
    if let Err(error) = start_result {
        eprintln!("Failed to start recorder: {}", error);
        std::process::exit(AUDIO_ERROR);
    }

    let sample_rate = recorder.sample_rate() as u32;
    let sample_rate_float = sample_rate as f64;

    let mut wav_writer = if !matches.get_flag("stream") {
        Some(create_wav_writer(&matches, sample_rate))
    } else {
        None
    };


    let mut silence_duration_ms = 0u64;

    // Initialize variables for the moving average calculation
    let mut rms_moving_average = 0.0f32;
    let mut dynamic_silence_threshold;

    if wav_writer.is_none() {
        let sample_rate_bytes = sample_rate.to_le_bytes();
        let mut handle = stdout().lock();
        write_to_stdout(&mut handle, &[BYTE_CURRENT_VERSION, BYTE_CONFIG]);
        write_to_stdout(&mut handle, &sample_rate_bytes);
    }

    while recorder.is_recording() {
        match recorder.read() {
            Ok(frame) => {
                let rms = calculate_rms(&frame);

                // Update the moving average of RMS values
                rms_moving_average = (ALPHA * rms) + ((1.0 - ALPHA) * rms_moving_average);

                // Adjust the dynamic silence threshold based on the moving average
                dynamic_silence_threshold = rms_moving_average * SILENCE_FACTOR;

                // Calculate frame duration in milliseconds
                let frame_duration_ms = (1000f64 * frame.len() as f64 / sample_rate_float) as u64;

                let is_silence = rms < dynamic_silence_threshold;

                if is_silence {
                    silence_duration_ms += frame_duration_ms;
                    if silence_threshold_ms > 0 && silence_duration_ms >= silence_threshold_ms {
                        eprintln!("Stopping recording due to silence.");
                        if let Err(error) = recorder.stop() {
                            eprintln!("Failed to stop recorder: {}", error);
                            std::process::exit(AUDIO_ERROR);
                        }
                        break;
                    }
                } else {
                    silence_duration_ms = 0; // Reset silence duration if noise is detected
                }

                if let Some(ref mut writer) = wav_writer {
                    for sample in &frame {
                        if let Err(error) = writer.write_sample(*sample) {
                            eprintln!("Failed to write sample: {}", error);
                            std::process::exit(FILE_ERROR);
                        }
                    }
                    if let Err(error) = writer.flush() {
                        eprintln!("Failed to flush wav writer: {}", error);
                        std::process::exit(FILE_ERROR);
                    }
                } else {
                    // Streaming mode: write raw audio data to stdout
                    let stdout = stdout();
                    let mut handle = stdout.lock();
                    write_to_stdout(&mut handle, &[BYTE_FRAME]);
                    write_to_stdout(&mut handle, &[is_silence as u8]);
                    write_to_stdout(&mut handle, &frame.len().to_le_bytes());
                    for sample in &frame {
                        let bytes = sample.to_le_bytes();
                        write_to_stdout(&mut handle, &bytes);
                    }
                }
            }
            Err(error) => {
                eprintln!("Failed to read frame: {}", error);
                std::process::exit(AUDIO_ERROR);
            }
        }
    }

    if let Some(mut writer) = wav_writer {
        if let Err(error) = writer.flush() {
            eprintln!("Failed to flush wav writer: {}", error);
            std::process::exit(FILE_ERROR);
        }
        if let Err(error) = writer.finalize() {
            eprintln!("Failed to finalize wav writer: {}", error);
            std::process::exit(FILE_ERROR);
        }
    }
    eprintln!("Done");
}

fn write_to_stdout(lock: &mut StdoutLock, bytes: &[u8]) {
    if let Err(error) = lock.write(&bytes) {
        eprintln!("Failed to write to stdout: {}", error);
        std::process::exit(FILE_ERROR);
    }
}

fn create_recorder_builder(matches: &ArgMatches) -> PvRecorderBuilder {
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

    eprintln!("Using library {}", library_path.to_string_lossy());

    recorder_builder.library_path(&library_path);
    recorder_builder
}

fn create_wav_writer(matches: &ArgMatches, sample_rate: u32) -> WavWriter<File> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };

    let wav_writer = if let Some(output_path) = matches.get_one::<PathBuf>("output") {
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
        match WavWriter::new(file, spec) {
            Ok(wav_writer) => wav_writer,
            Err(error) => {
                eprintln!("Failed to create wav writer: {}", error);
                std::process::exit(AUDIO_ERROR);
            }
        }
    } else {
        eprintln!("No output file specified");
        std::process::exit(USER_ERROR);
    };
    wav_writer
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
    return Ok(library_path);
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

// Function to calculate the RMS value of an audio frame
fn calculate_rms(frame: &[i16]) -> f32 {
    let sum_of_squares: i64 = frame.iter().map(|&sample| (sample as i64).pow(2)).sum();
    let mean_square = sum_of_squares as f32 / frame.len() as f32;
    mean_square.sqrt()
}
