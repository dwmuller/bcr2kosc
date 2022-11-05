use std::time::Duration;
use std::{error::Error, net::SocketAddr};

use clap::{Parser, Subcommand};
use futures::{pin_mut, select, FutureExt, SinkExt, Stream, StreamExt};
use log::info;
use midi_control::message::SysExType;
use midi_control::{MidiMessage, SysExEvent};

mod b_control;
mod midi_io;
use b_control::*;
mod bcl;
mod midi_util;
mod osc_service;
use osc_service::*;
use tokio::signal;

use crate::midi_io::{MidiSink, MidiStream};
mod translator;

pub const PGM: &str = "bcr2kosc";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {

    /// Logging verbosity. Specify multiple times for more verbosity, e.g. -vvv.
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

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
        #[arg(default_value_t = 1)]
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

    let cli = Cli::parse();
    stderrlog::new().verbosity(cli.verbose as usize).init().unwrap();
    match &cli.command {
        Some(Commands::List {}) => Ok(list_ports()),
        Some(Commands::Listen { midi_in }) => listen(midi_in).await,
        Some(Commands::Info {
            midi_in,
            midi_out,
            device,
        }) => info(midi_in, midi_out, *device),
        Some(Commands::Find { midi_in, midi_out }) => list_bcontrols(midi_in, midi_out).await,
        Some(Commands::Serve {
            midi_in,
            midi_out,
            osc_in_addr,
            osc_out_addrs,
        }) => serve(&midi_in, &midi_out, &osc_in_addr, &osc_out_addrs).await,
        None => Ok(()),
    }
}

fn list_ports() {
    fn print_ports(dir: &str, lst: &[String]) {
        match lst.len() {
            0 => println!("No {dir} ports found"),
            _ => {
                println!("\nAvailable {dir} ports:");
                for (i, p) in lst.iter().enumerate() {
                    println!("{i}: {p}");
                }
            }
        };
    }

    print_ports("input", &midi_io::input_ports());
    print_ports("output", &midi_io::output_ports());
}

async fn listen(port_name: &str) -> Result<(), Box<dyn Error>> {
    async fn print_midi_input(midi_in: impl Stream<Item = MidiMessage>) {
        pin_mut!(midi_in);
        while let Some(msg) = midi_in.next().await {
            println!("{msg:?}");
        }
    }

    let midi_in = MidiStream::bind(port_name)?;
    select! {
        _ = print_midi_input(midi_in).fuse() => {},
        _ = signal::ctrl_c().fuse() => {}
    };
    Ok(())
}

fn info(_in_port_name: &str, _out_port_name: &str, _device: u8) -> Result<(), Box<dyn Error>> {
    todo!();
}

async fn list_bcontrols(in_port_name: &str, out_port_name: &str) -> Result<(), Box<dyn Error>> {
    async fn listen_for_sysex(midi_in: MidiStream) {
        pin_mut!(midi_in);
        while let Some(msg) = midi_in.next().await {
            if let MidiMessage::SysEx(SysExEvent {
                r#type: SysExType::Manufacturer(BEHRINGER),
                data,
            }) = msg
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
        }
    }
    let midi_in = MidiStream::bind(in_port_name)?;
    let midi_out = MidiSink::bind(out_port_name)?;

    let bdata = BControlSysEx {
        device: DeviceID::Device(0),
        model: Some(BControlModel::BCR),
        command: BControlCommand::RequestIdentity,
    }
    .to_midi();
    let req = MidiMessage::SysEx(SysExEvent {
        r#type: SysExType::Manufacturer(BEHRINGER),
        data: bdata,
    });
    pin_mut!(midi_out);
    midi_out.send(req).await?;
    select! {
        _ = listen_for_sysex(midi_in).fuse() => {},
        _ = tokio::time::sleep(Duration::from_millis(100)).fuse() => {},
    }
    Ok(())
}

async fn serve(
    midi_in: &str,
    midi_out: &str,
    osc_in_addr: &SocketAddr,
    osc_out_addrs: &[SocketAddr],
) -> Result<(), Box<dyn Error>> {
    {
        let mut svc = BCtlOscSvc::new(midi_in, midi_out, osc_in_addr, osc_out_addrs);
        select! {
            _ = svc.run().fuse() => {info!("Stopped.");},
            _ = signal::ctrl_c().fuse() => {svc.stop().await; },
        };
        Ok(())
    }
}
