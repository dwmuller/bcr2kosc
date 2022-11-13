#![deny(unused_crate_dependencies)]
#![deny(missing_docs)]

//! A service to translate between MIDI and OSC, specifically targeting
//! Behringer B-Controllers (the B-Control Rotary and B-Control Faderport).
//!
use std::time::Duration;
use std::{error::Error, net::SocketAddr};

use clap::{Parser, Subcommand};
use futures::{pin_mut, select, FutureExt, SinkExt, Stream, StreamExt};
use log::info;
use midi_control::MidiMessage;
use tokio::signal;

mod b_control;
mod bcl;
mod midi_io;
mod osc_service;
mod translator;

use crate::b_control::*;
use crate::midi_io::{MidiSink, MidiStream};
use crate::osc_service::*;

/// Program name, used in a variety of log messages.
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
    ListPorts {},
    /// Listen to a port and display received MIDI.
    ///
    /// Useful for debugging.
    Listen {
        /// The name of the port to listen to. Use the list command to see ports.
        midi_in: String,
    },
    /// Find and list Behringer B-Control devices.
    Find {
        /// Time delay to listen for a response before giving up, in seconds.
        #[arg(long, default_value_t = 1)]
        delay: u64,
        /// The name of the MIDI port recieve data from.
        midi_in: String,
        /// The name of the MIDI port to send data to.
        midi_out: String,
    },
    /// Get global settings BCL from a B-Control.
    GetGlobal {
        /// The name of the MIDI port recieve data from.
        midi_in: String,
        /// The name of the MIDI port to send data to.
        midi_out: String,
        /// The device number of the B-Control, which can be one through 16.
        /// Defaults to 1.
        #[arg(default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=16))]
        device: u8,
    },
    /// Get preset information from a B-Control.
    GetPreset {
        /// The name of the MIDI port recieve data from.
        midi_in: String,
        /// The name of the MIDI port to send data to.
        midi_out: String,
        /// The device number of the B-Control, which can be one through 16.
        /// Defaults to 1.
        #[arg(default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=16))]
        device: u8,
        /// The number of the preset to retrieve, from 1 to 32, "temp", or
        /// "all". Defaults to temp.
        ///
        /// If you specify "all", you get a dump of the device's global
        /// settings, followed by all filled memory presets,
        #[arg(default_value_t = PresetIndex::Temporary, value_parser=parse_preset_arg)]
        preset: PresetIndex,
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
fn parse_preset_arg(s: &str) -> Result<PresetIndex> {
    match s {
        "all" => Ok(PresetIndex::All),
        "temp" => Ok(PresetIndex::Temporary),
        _ => {
            let num = s.parse::<u8>().ok();
            match num {
                Some(n) if (1u8..=32u8).contains(&n) => Ok(PresetIndex::Preset(n - 1)),
                _ => Err(LocalError::from("invalid preset index")),
            }
        }
    }
}
type LocalError = Box<dyn Error + Send + Sync + 'static>;
type Result<T> = std::result::Result<T, LocalError>;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    stderrlog::new()
        .verbosity(cli.verbose as usize)
        .init()
        .unwrap();
    match &cli.command {
        Some(Commands::ListPorts {}) => Ok(list_ports()),
        Some(Commands::Listen { midi_in }) => listen(midi_in).await,
        Some(Commands::GetGlobal {
            midi_in,
            midi_out,
            device,
        }) => get_global(midi_in, midi_out, *device).await,
        Some(Commands::GetPreset {
            midi_in,
            midi_out,
            device,
            preset,
        }) => get_preset(midi_in, midi_out, *device, *preset).await,
        Some(Commands::Find {
            delay,
            midi_in,
            midi_out,
        }) => list_bcontrols(midi_in, midi_out, *delay).await,
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

async fn listen(port_name: &str) -> Result<()> {
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

async fn get_global(in_port_name: &str, out_port_name: &str, device: u8) -> Result<()> {
    let mut midi_in = MidiStream::bind(in_port_name)?;
    let mut midi_out = MidiSink::bind(out_port_name)?;
    for line in get_global_bcl(device - 1, &mut midi_in, &mut midi_out).await? {
        println!("{line}");
    }
    Ok(())
}

async fn get_preset(
    in_port_name: &str,
    out_port_name: &str,
    device: u8,
    preset: PresetIndex,
) -> Result<()> {
    let mut midi_in = MidiStream::bind(in_port_name)?;
    let mut midi_out = MidiSink::bind(out_port_name)?;
    for line in get_preset_bcl(device - 1, preset, &mut midi_in, &mut midi_out).await? {
        println!("{line}")
    }
    Ok(())
}

async fn list_bcontrols(in_port_name: &str, out_port_name: &str, delay: u64) -> Result<()> {
    let timeout = tokio::time::sleep(Duration::from_secs(delay));
    let midi_in = MidiStream::bind(in_port_name)?
        .filter_map(|m| async move { BControlSysEx::try_from(&m).ok() })
        .take_until(timeout);

    let bdata = BControlSysEx {
        device: DeviceID::Any,
        model: BControlModel::Any,
        command: BControlCommand::RequestIdentity,
    };
    let action = |sysex| async {
        if let BControlSysEx {
            device: DeviceID::Device(dev),
            model,
            command: BControlCommand::SendIdentity { id_string },
        } = sysex
        {
            let dev = dev + 1;
            println!("{dev}, {model:}, {id_string}");
        }
    };
    MidiSink::bind(out_port_name)?
        .send(MidiMessage::from(&bdata))
        .await?;
    midi_in.for_each(action).await;
    Ok(())
}

async fn serve(
    midi_in: &str,
    midi_out: &str,
    osc_in_addr: &SocketAddr,
    osc_out_addrs: &[SocketAddr],
) -> Result<()> {
    {
        let mut svc = BCtlOscSvc::new(midi_in, midi_out, osc_in_addr, osc_out_addrs);
        select! {
            _ = svc.run().fuse() => {info!("Stopped.");},
            _ = signal::ctrl_c().fuse() => {svc.stop().await; },
        };
        Ok(())
    }
}
