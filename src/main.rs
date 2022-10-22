use clap::{Parser, Subcommand};
use log::info;
use stderrlog::LogLevelNum;
use std::io::stdin;
use std::time::Duration;
use std::{error::Error, net::SocketAddr};

use midi_msg::{MidiMsg, SystemExclusiveMsg};
use midir::{MidiIO, MidiInput, MidiOutput};

mod b_control;
mod bcl;
use b_control::*;
mod midi_util;
use midi_util::*;
mod osc_service;

pub const PGM: &str = "bcr2kosc";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List MIDI ports.
    List {},
    /// Listen to a port and display received MIDI. Useful for debugging.
    Listen {
        /// The name of the port to listen to. Use the list command to see ports.
        port_name: String,
    },
    /// Find and list Behringer B-Control devices.
    Find {
        /// The name of the MIDI port recieve data from.
        in_port_name: String,
        /// The name of the MIDI port to send data to.
        out_port_name: String,
    },
    /// Start an OSC service/client pair that translates to and from MIDI.
    Serve {
        /// The name of the input MIDI port.
        midi_in: String,
        /// The name of the output MIDI port.
        midi_out: String,
        /// The address and port on which to listen for OSC via UDP.
        osc_in_addr: SocketAddr,
        /// The addresses from which to accept OSC and to which OSC will be
        /// sent.
        osc_out_addrs: Vec<SocketAddr>,
    },
}


#[async_std::main]
async fn main() -> Result<(), Box<dyn Error>> {
    stderrlog::new().verbosity(LogLevelNum::Debug).init().unwrap();

    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::List {}) => Ok(list_ports()),
        Some(Commands::Listen { port_name }) => listen(port_name),
        Some(Commands::Find {
            in_port_name,
            out_port_name,
        }) => list_bcontrols(in_port_name, out_port_name),
        Some(Commands::Serve {
            midi_in,
            midi_out,
            osc_in_addr,
            osc_out_addrs,
        }) => {
            let svc = 
                osc_service::start(midi_in, midi_out, osc_in_addr, osc_out_addrs)
                    .await?;
            wait_for_user()?;
            svc.stop().await;
            info!("Stopped.");
            Ok(())
        }
        None => Ok(()),
    }
}

fn list_ports() {
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
    let _conn_in = midi_in.connect(
        &in_port,
        &format!("{PGM} listen connection"),
        move |stamp, msg, _| {
            if let Ok((midi, _)) = MidiMsg::from_midi(msg) {
                println!("{stamp}: {midi:?} (len={})", msg.len());
            }
        },
        (),
    )?;
    println!("Connection open, reading input from '{port_name}'.");
    wait_for_user()?;
    Ok(())
}

fn wait_for_user() -> Result<(), Box<dyn Error>> {
    println!("Press Enter to exit.");
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    Ok(())
}

fn list_bcontrols(in_port_name: &str, out_port_name: &str) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiInput::new(&format!("{PGM} finding B-controls"))?;
    let in_port = find_port(&midi_in, in_port_name)?;
    let _conn_in = midi_in.connect(
        &in_port,
        in_port_name,
        move |_stamp, midi_data, _context| {
            let mm = MidiMsg::from_midi(midi_data);
            if let Ok((
                MidiMsg::SystemExclusive {
                    msg:
                        SystemExclusiveMsg::Commercial {
                            id: BEHRINGER,
                            data,
                        },
                },
                _, // unused size of consumed MIDI
            )) = mm
            {
                // Recognized as a Behringer sysex. Parse the sysex payload.
                let bc = BControlSysEx::from_midi(&data);
                if let Ok((
                    BControlSysEx {
                        device: DeviceID::Device(dev),
                        model: _,
                        command: BControlCommand::SendIdentity { id_string },
                    },
                    _, // unused size of consumed data
                )) = bc
                {
                    println!("{}: {}", dev, id_string);
                }
            }
        },
        (),
    )?;

    let bdata = BControlSysEx {
        device: DeviceID::Device(0),
        model: Some(BControlModel::BCR),
        command: BControlCommand::RequestIdentity,
    }
    .to_midi();
    let req = MidiMsg::SystemExclusive {
        msg: (SystemExclusiveMsg::Commercial {
            id: BEHRINGER,
            data: bdata,
        }),
    }
    .to_midi();
    let midi_out = MidiOutput::new(&format!("{PGM} finding B-controls"))?;
    let out_port = find_port(&midi_out, out_port_name)?;
    let mut conn_out = midi_out.connect(&out_port, "{PGM} finding B-controls")?;
    conn_out.send(&req)?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
