
use clap::{Parser, Subcommand};
use std::io::{stdin/* , stdout, Write */};
use std::error::Error;

use midir::{MidiInput, MidiInputPort};

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
        Some(Commands::List {}) => list_ports(),
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

fn list_ports() -> Result<(), Box<dyn Error>> {

    let midi_in = MidiInput::new("bcr2kosc listing")?;
    let in_ports = midi_in.ports();
    match in_ports.len() {
        0 => return Err("no input ports found".into()),
        _ => {
            println!("\nAvailable input ports:");
            for (i, p) in in_ports.iter().enumerate() {
                println!("{}: {}", i, midi_in.port_name(p).unwrap());
            }
        }
    };
    Ok(())
}

fn listen(port_name: &str) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new("bcr2kosc listening")?;
    let in_port = find_port(&midi_in, port_name)?;
    let _conn_in = midi_in.connect(&in_port, "bcr2kosc listen connection", 
        move |stamp, msg, _| {
              println!("{}: {:?} (len={})", stamp, msg, msg.len());
        }, ())?;
    println!("Connection open, reading input from '{}'. Press Enter to exit.", 
             port_name);
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    println!("Closing connection");
    Ok(())
}