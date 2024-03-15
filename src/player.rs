use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use rodio::{Decoder, OutputStream, Sink};

pub fn play(file: &PathBuf) {
    let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&stream_handle).unwrap();
    let file = File::open(file).unwrap();
    let reader = BufReader::new(file);
    let source = Decoder::new(reader).unwrap();
    sink.append(source);
    sink.sleep_until_end();
}