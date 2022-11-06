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

mod osc_service;
use osc_service::*;
use tokio::signal;

use crate::midi_io::{MidiSink2, MidiStream};
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
        /// Time delay to listen for a response before giving up.
        #[arg(long, default_value_t = 1)]
        delay: u64,
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
        /// Time delay to listen for a response before giving up.
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
            delay,
            midi_in,
            midi_out,
            device,
        }) => info(midi_in, midi_out, *device, *delay).await,
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

struct Spawner {}
impl futures::task::Spawn for Spawner {
    fn spawn_obj(
        &self,
        future: futures::task::FutureObj<'static, ()>,
    ) -> Result<(), futures::task::SpawnError> {
        tokio::task::spawn(future);
        Ok(())
    }
}

async fn info(
    in_port_name: &str,
    out_port_name: &str,
    device: u8,
    _delay: u64,
) -> Result<(), Box<dyn Error>> {
    async fn listen_for_bcl(in_port_name: &str, device: u8) -> Result<(), Box<dyn Error>> {
        let midi_in = MidiStream::bind(in_port_name)?.filter_map(|m| async {
            match m {
                MidiMessage::SysEx(SysExEvent {
                    r#type: SysExType::Manufacturer(BEHRINGER),
                    data,
                }) => {
                    let bc = BControlSysEx::from_midi(&data);
                    if let Ok((
                        BControlSysEx {
                            device: DeviceID::Device(dev),
                            model: _,
                            command: BControlCommand::SendBclMessage { text },
                        },
                        _,
                    )) = bc
                    {
                        if dev != device {
                            None
                        } else {
                            Some(text)
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            }
        });
        pin_mut!(midi_in);
        while let Some(bcl) = midi_in.next().await {
            println!("{bcl:?}");
        }
        Ok(())
    }
    let midi_out = MidiSink2::bind(out_port_name, Spawner {})?;
    // let midi_out = MidiSink2::bind(out_port_name)?;

    let bdata = BControlSysEx {
        device: DeviceID::Any,
        model: BControlModel::Any,
        command: BControlCommand::RequestData(PresetIndex::Temporary),
    };
    let req = to_midi_sysex(bdata);
    pin_mut!(midi_out);
    midi_out.send(req).await?;

    listen_for_bcl(in_port_name, device).await?;
    Ok(())
}

async fn list_bcontrols(
    in_port_name: &str,
    out_port_name: &str,
    delay: u64,
) -> Result<(), Box<dyn Error>> {
    let midi_in = MidiStream::bind(in_port_name)?;
    let midi_out = MidiSink2::bind(out_port_name, Spawner {})?;
    // let midi_out = MidiSink2::bind(out_port_name)?;

    let timeout = tokio::time::sleep(Duration::from_secs(delay));
    let midi_in = midi_in
        .filter_map(|m| filter_behringer_sysex(m))
        .take_until(timeout);

    let bdata = BControlSysEx {
        device: DeviceID::Any,
        model: BControlModel::Any,
        command: BControlCommand::RequestIdentity,
    };
    let req = to_midi_sysex(bdata);
    pin_mut!(midi_out);
    midi_out.send(req).await?;
    let f = |sysex| async {
        if let BControlSysEx {
            device: DeviceID::Device(dev),
            model,
            command: BControlCommand::SendIdentity { id_string },
        } = sysex
        {
            println!("{dev}, {model:}, {id_string}");
        }
    };
    midi_in.for_each(f).await;
    Ok(())
}

async fn filter_behringer_sysex(msg: MidiMessage) -> Option<BControlSysEx> {
    if let MidiMessage::SysEx(SysExEvent {
        r#type: SysExType::Manufacturer(BEHRINGER),
        data,
    }) = msg
    {
        // Recognized as a Behringer sysex. Parse the sysex payload.
        match BControlSysEx::from_midi(&data) {
            Ok(bcse) => Some(bcse.0),
            Err(_) => None,
        }
    } else {
        None
    }
}

fn to_midi_sysex(bdata: BControlSysEx) -> MidiMessage {
    let bdata = bdata.to_midi();
    let req = MidiMessage::SysEx(SysExEvent {
        r#type: SysExType::Manufacturer(BEHRINGER),
        data: bdata,
    });
    req
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
