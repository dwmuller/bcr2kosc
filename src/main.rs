use clap::{Parser, Subcommand};
use log::info;
use midi_control::message::SysExType;
use midi_control::{MidiMessage, SysExEvent};
use std::io::stdin;
use std::time::Duration;
use std::{error::Error, net::SocketAddr};
use stderrlog::LogLevelNum;

use midir::{MidiIO, MidiInput, MidiOutput};

mod b_control;
mod midi_io;
use b_control::*;
mod bcl;
mod midi_util;
use midi_util::*;
mod osc_service;
use osc_service::*;
mod translator;

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
        midi_in: String,
    },
    /// List information about a specific B-Control.
    Info {
        /// The name of the MIDI port recieve data from.
        midi_in: String,
        /// The name of the MIDI port to send data to.
        midi_out: String,
        /// The device number of the B-Control, which can be one through 16.
        /// Defaults to 1. 
        #[arg(default_value_t=1)]
        device: u8,
    },
    /// Find and list Behringer B-Control devices.
    Find {
        /// The name of the MIDI port recieve data from.
        midi_in: String,
        /// The name of the MIDI port to send data to.
        midi_out: String,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    stderrlog::new()
        .verbosity(LogLevelNum::Debug)
        .init()
        .unwrap();

    let cli = Cli::parse();
    match &cli.command {
        Some(Commands::List {}) => Ok(list_ports()),
        Some(Commands::Listen { midi_in }) => listen(midi_in),
        Some(Commands::Info {midi_in, midi_out, device}) => info(midi_in, midi_out, *device),
        Some(Commands::Find {
            midi_in,
            midi_out,
        }) => list_bcontrols(midi_in, midi_out),
        Some(Commands::Serve {
            midi_in,
            midi_out,
            osc_in_addr,
            osc_out_addrs,
        }) => {
            let mut svc = BCtlOscSvc::new(midi_in, midi_out, osc_in_addr, osc_out_addrs);
            svc.start().await?;
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
            let midi = MidiMessage::from(msg);
            println!("{stamp}: {midi:?} (len={})", msg.len());
        },
        (),
    )?;
    println!("Connection open, reading input from '{port_name}'.");
    wait_for_user()?;
    Ok(())
}

fn info(_in_port_name: &str, _out_port_name: &str, _device: u8)  -> Result<(), Box<dyn Error>> {
    todo!();
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
            let mm = MidiMessage::from(midi_data);
            if let MidiMessage::SysEx(SysExEvent {
                r#type: SysExType::Manufacturer(BEHRINGER),
                data,
            }) = mm
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
    let req = Vec::<u8>::from(MidiMessage::SysEx(SysExEvent {
        r#type: SysExType::Manufacturer(BEHRINGER),
        data: bdata,
    }));
    let midi_out = MidiOutput::new(&format!("{PGM} finding B-controls"))?;
    let out_port = find_port(&midi_out, out_port_name)?;
    let mut conn_out = midi_out.connect(&out_port, "{PGM} finding B-controls")?;
    conn_out.send(&req)?;
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}
