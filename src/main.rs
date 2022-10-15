
use clap::{Parser, Subcommand};
use std::io::{stdin/* , stdout, Write */};
use std::error::Error;

use midir::{MidiIO, MidiInput, MidiInputPort, MidiOutput};
use midi_msg::MidiMsg;

const PGM :&str = "bcr2kosc";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>
}

#[derive(Subcommand)]
enum Commands {
    /// List MIDI ports.
    List {},
    /// Listen to a port and display received MIDI. Useful for debugging.
    Listen {
        /// The name of the port to listen to. Use the list command to see ports.
        port_name: String
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::List {}) => { Ok(list_ports()) },
        Some(Commands::Listen{ port_name }) => listen(port_name),
        None => Ok(())
    }
}

fn find_port(midi_in: &MidiInput, port_name: &str) -> Result<MidiInputPort, Box<dyn Error>> {
    let ports = midi_in.ports();
    let wanted = Ok(port_name.to_string());
    let port = ports.iter()
        .find(|&x| midi_in.port_name(&x) == wanted);
    match port {
        Some(p) => Ok(p.clone()),
        None => Err("MIDI port not found".into()),
    }
}

fn list_ports () {
    let midi_in = MidiInput::new("{PGM} list_ports").unwrap();
    print_ports("input", &midi_in);
    let midi_out = MidiOutput::new("{PGM} list_ports").unwrap();
    print_ports("output", &midi_out);
}

fn print_ports<T: MidiIO>(dir: &str, io: &T) {

    let ports = io.ports();
    match ports.len() {
        0 => println!("No {dir} ports found"),
        _ => {
            println!("\nAvailable {dir} ports:");
            for (i, p) in ports.iter().enumerate() {
                println!("{i}: {}", io.port_name(p).unwrap());
            }
        }
    };
}

fn listen(port_name: &str) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new(&format!("{PGM} listening"))?;
    let in_port = find_port(&midi_in, port_name)?;
    let _conn_in = midi_in.connect(&in_port, &format!("{PGM} listen connection"), 
        move |stamp, msg, _| {
            if let Ok(midi) = MidiMsg::from_midi(msg) {
                println!("{stamp}: {midi:?} (len={})", msg.len());
            }
        }, ())?;
    println!("Connection open, reading input from '{port_name}'. Press Enter to exit.");
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    println!("Closing connection");
    Ok(())
}

