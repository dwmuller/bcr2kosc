use clap::{Parser, Subcommand};
use std::io::{stdin/* , stdout, Write */};
use std::error::Error;
use std::time::Duration;

use midir::{MidiIO, MidiInput, MidiOutput};
use midi_msg::{MidiMsg, SystemExclusiveMsg};

mod b_control;
use b_control::*;

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
    },
    /// Find and list Behringer B-Control devices.
    Find {
        /// The name of the MIDI port recieve data from.
        in_port_name: String,
        /// The name of the MIDI port to send data to.
        out_port_name: String,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::List {}) => Ok(list_ports()),
        Some(Commands::Listen{ port_name }) => listen(port_name),
        Some(Commands::Find {in_port_name, out_port_name }) 
            => list_bcontrols(in_port_name, out_port_name),
        None => Ok(())
    }
}

fn find_port<T: MidiIO>(midi_io: &T, port_name: &str) -> Result<T::Port, Box<dyn Error>> {
    let ports = midi_io.ports();
    let wanted = Ok(port_name.to_string());
    let port = ports.iter()
        .find(|&x| midi_io.port_name(&x) == wanted);
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

fn print_ports(dir: &str, io: &impl MidiIO) {

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
            if let Ok((midi, _)) = MidiMsg::from_midi(msg) {
                println!("{stamp}: {midi:?} (len={})", msg.len());
            }
        }, ())?;
    println!("Connection open, reading input from '{port_name}'. Press Enter to exit.");
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    println!("Closing connection");
    Ok(())
}

fn list_bcontrols(in_port_name: &str, out_port_name: &str) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new(&format!("{PGM} finding B-controls"))?;
    let in_port = find_port(&midi_in, in_port_name)?;
    let _conn_in = midi_in.connect(&in_port, in_port_name, 
        move |_stamp, midi_data, _context| {
            let mm = MidiMsg::from_midi(midi_data);
            if let Ok((
                    MidiMsg::SystemExclusive {
                        msg: SystemExclusiveMsg::Commercial {
                            id: BEHRINGER, data
                        }
                    },
                    _ // unused size of consumed MIDI
                )) = mm {
                    // Recognized as a Behringer sysex. Parse the sysex payload.
                    let bc = BControlSysEx::from_midi(&data);
                    if let Ok((
                            BControlSysEx{
                                device: DeviceID::Device(dev), 
                                model: _,
                                command: BControlCommand::SendIdentity { id_string }
                            },
                            _ // unused size of consumed data
                        )) = bc {
                            println!("{}: {}", dev, id_string);
                    }
                }
            },
        ())?;

    let bdata = BControlSysEx{
        device: DeviceID::Device(0), 
        model: BControlModel::BCR, 
        command: BControlCommand::RequestIdentity
    }.to_midi();
    let req = MidiMsg::SystemExclusive {
        msg: (SystemExclusiveMsg::Commercial { id: BEHRINGER, data: bdata })
    }.to_midi();
    let midi_out = MidiOutput::new(&format!("{PGM} finding B-controls"))?;
    let out_port = find_port(&midi_out, out_port_name)?;
    let mut conn_out = midi_out.connect(&out_port, "{PGM} finding B-controls")?;
    conn_out.send(&req)?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

