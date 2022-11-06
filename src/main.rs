#![deny(missing_docs)]

//! A service to translate between MIDI and OSC, specifically targeting
//! Behringer B-Controllers (the B-Control Rotary and B-Control Faderport).
//! 
use std::task::Poll;
use std::time::Duration;
use std::{error::Error, net::SocketAddr};

use clap::{Parser, Subcommand};
use futures::{future, pin_mut, select, FutureExt, SinkExt, Stream, StreamExt};
use log::info;
use midi_control::MidiMessage;

mod b_control;
mod midi_io;
use b_control::*;
mod bcl;

mod osc_service;
use osc_service::*;
use tokio::signal;

use crate::midi_io::{MidiSink, MidiStream};
mod translator;

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
    List {},
    /// Listen to a port and display received MIDI.
    /// 
    /// Useful for debugging.
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
        #[arg(default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=16))]
        device: u8,
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
    stderrlog::new()
        .verbosity(cli.verbose as usize)
        .init()
        .unwrap();
    match &cli.command {
        Some(Commands::List {}) => Ok(list_ports()),
        Some(Commands::Listen { midi_in }) => listen(midi_in).await,
        Some(Commands::Info {
            midi_in,
            midi_out,
            device,
        }) => info(midi_in, midi_out, *device - 1).await,
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

async fn info(in_port_name: &str, out_port_name: &str, device: u8) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiStream::bind(in_port_name)?
        .filter_map(|m| async { BControlSysEx::try_from(m).ok() })
        .filter_map(|sysex| {
            future::ready(if !sysex.device.match_device(device) {
                None
            } else if let BControlCommand::SendBclMessage { msg_index, text } = sysex.command {
                Some((msg_index, text))
            } else {
                None
            })
        });
    let done = std::sync::Mutex::new(false);
    let stop_fut = future::poll_fn(|_cx| {
        if *done.lock().unwrap() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    });
    let midi_in = midi_in.take_until(stop_fut).inspect(|(_, text)| {
        *done.lock().unwrap() = text == "$end";
    });

    let bdata = BControlSysEx {
        device: DeviceID::Device(device),
        model: BControlModel::Any,
        command: BControlCommand::RequestData(PresetIndex::Temporary),
    };
    MidiSink::bind(out_port_name)?
        .send(MidiMessage::from(bdata))
        .await?;
    let mut lines: Vec<(u16, String)> = midi_in.collect().await;
    lines.sort_by_key(|item| item.0);
    let lines: Vec<String> = lines.drain(..).map(|item| item.1).collect();
    for line in lines {
        println!("{line}")
    }
    // midi_in.for_each(|(n, text)| future::ready(println!("{n:06} {text}"))).await;
    Ok(())
}

async fn list_bcontrols(
    in_port_name: &str,
    out_port_name: &str,
    delay: u64,
) -> Result<(), Box<dyn Error>> {
    let timeout = tokio::time::sleep(Duration::from_secs(delay));
    let midi_in = MidiStream::bind(in_port_name)?
        .filter_map(|m| async { BControlSysEx::try_from(m).ok() })
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
            println!("{dev}, {model:}, {id_string}");
        }
    };
    MidiSink::bind(out_port_name)?
        .send(MidiMessage::from(bdata))
        .await?;
    midi_in.for_each(action).await;
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
