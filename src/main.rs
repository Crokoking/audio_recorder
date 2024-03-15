use std::env;
use std::path::PathBuf;

use clap::{Arg, ArgAction, command, value_parser};

use audio_recorder::recorder;

static USER_ERROR: i32 = 2;


fn main() {
    let matches = command!() // requires `cargo` feature
        .arg(Arg::new("device").short('d').long("device").required(false).value_parser(value_parser!(i32)))
        .arg(Arg::new("output").short('o').long("output").required(false).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("stream").long("stream").required(false).action(ArgAction::SetTrue).help("Stream audio to stdout instead of writing to a file"))
        .arg(Arg::new("lib").long("lib").required(false).value_parser(value_parser!(PathBuf)))
        .arg(Arg::new("play").short('p').long("play").required(false).value_parser(value_parser!(PathBuf)).help("Play a file instead of recording"))
        .arg(Arg::new("list").short('l').long("list").required(false).action(ArgAction::SetTrue))
        .arg(Arg::new("stop-silence").short('s').long("stop-silence").required(false).value_parser(value_parser!(u64)).help("Stop recording after this many milliseconds of silence"))
        .get_matches();

    if matches.get_one::<PathBuf>("play").is_some() {
        let file = matches.get_one::<PathBuf>("play").unwrap();
        audio_recorder::player::play(file);
        return;
    }

    if matches.get_flag("stream") && matches.get_one::<PathBuf>("output").is_some() {
        eprintln!("Cannot specify both --stream and --output");
        std::process::exit(USER_ERROR);
    }

    if !matches.get_flag("stream") && matches.get_one::<PathBuf>("output").is_none() {
        eprintln!("Must specify either --stream or --output");
        std::process::exit(USER_ERROR);
    }

    let lib = matches.get_one::<PathBuf>("lib");
    let device = matches.get_one::<i32>("device");
    let output = matches.get_one::<PathBuf>("output");
    let stop_silence = matches.get_one::<u64>("stop-silence");


    if matches.get_flag("list") {
        recorder::list_devices(lib);
        return;
    }

    recorder::record(device, output, lib, stop_silence);
}