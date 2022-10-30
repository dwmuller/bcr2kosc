use midir::MidiIO;
use std::error::Error;

pub fn find_port<T: MidiIO>(midi_io: &T, port_name: &str) -> Result<T::Port, Box<dyn Error>> {
    let ports = midi_io.ports();
    let wanted = Ok(port_name.to_string());
    let port = ports.iter().find(|&x| midi_io.port_name(&x) == wanted);
    match port {
        Some(p) => Ok(p.clone()),
        None => Err("MIDI port not found".into()),
    }
}
