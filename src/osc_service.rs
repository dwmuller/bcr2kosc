#![allow(unused_variables)]
//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to a configured UDP port.

use async_osc::{OscMessage, OscPacket, OscSender, OscSocket, OscType};
use async_std::channel::{self, Sender};
use async_std::net::SocketAddr;
use async_std::stream::StreamExt;
use async_std::sync::{Arc, Condvar, Mutex};
use async_std::task::{self, spawn, JoinHandle};

use log::{error, info, warn};
use midi_msg::{Channel, ChannelVoiceMsg, ControlChange, MidiMsg};
use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputPort};
use rosc::address::{Matcher, OscAddress};
use std::error::Error;

use crate::midi_util::*;
use crate::PGM;

pub struct BCtlOscSvc {
    pub midi_in_port_name: String,
    pub midi_out_port_name: String,
    pub osc_in_addr: SocketAddr,
    pub osc_out_addrs: Arc<Vec<SocketAddr>>,

    running: Arc<(Mutex<bool>, Condvar)>,
    spawned_tasks: Vec<JoinHandle<()>>,
}
impl BCtlOscSvc {
    pub fn new(
        midi_in_port_name: &str,
        midi_out_port_name: &str,
        osc_in_addr: &SocketAddr,
        osc_out_addrs: &[SocketAddr],
    ) -> Self {
        BCtlOscSvc {
            midi_in_port_name: midi_in_port_name.to_string(),
            midi_out_port_name: midi_out_port_name.to_string(),
            osc_in_addr: osc_in_addr.clone(),
            osc_out_addrs: Arc::new(osc_out_addrs.to_vec()),
            running: Arc::new((Mutex::new(false), Condvar::new())),
            spawned_tasks: Vec::new(),
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        (*self.running.0.lock().await) = true;
        {
            let running = self.running.clone();
            let midi_input = MidiInput::new(&format!("{PGM} listening to B-Control"))?;
            let midi_input_port = find_port(&midi_input, &self.midi_in_port_name)?;
            let osc_out_addrs = self.osc_out_addrs.clone();
            let osc_out_sock = OscSocket::bind("127.0.0.1:0").await?;
            self.spawned_tasks.push(spawn(async move {
                run_midi_listener(
                    running,
                    midi_input,
                    midi_input_port,
                    osc_out_sock,
                    osc_out_addrs,
                )
                .await;
            }));
        }
        let (s, r) = channel::bounded(10);
        {
            let running = self.running.clone();
            let osc_in_sock = OscSocket::bind(self.osc_in_addr).await?;
            self.spawned_tasks.push(spawn(async move {
                run_osc_listener(running, osc_in_sock, s).await;
            }));
        }
        {
            let midi_output = MidiOutput::new(&format!("{PGM} feedback to B-Control"))?;
            let midi_output_port = find_port(&midi_output, &self.midi_out_port_name)?;
            self.spawned_tasks
                .push(spawn(run_midi_sender(r, midi_output, midi_output_port)));
        }
        Ok(())
    }

    pub async fn stop(&mut self) {
        let (lock, cvar) = &*self.running;
        {
            let mut running = lock.lock().await;
            *running = false;
            cvar.notify_all();
        }
        for ele in self.spawned_tasks.drain(0..) {
            // Seems to pend if the task completed. Might have to live with
            // .cancel instead if this becomes a problem.
            ele.await;
        }
    }
}

async fn run_midi_listener(
    running: Arc<(Mutex<bool>, Condvar)>,
    midi_input: MidiInput,
    midi_input_port: MidiInputPort,
    osc_out_sock: OscSocket,
    osc_out_addrs: Arc<Vec<SocketAddr>>,
) {
    let midi_input_cxn = midi_input
        .connect(
            &midi_input_port,
            &format!("{PGM} listener"),
            move |t, m, _| {
                let mc = m.to_vec();
                let os = osc_out_sock.sender();
                let oa = osc_out_addrs.clone();
                task::spawn(async move {
                    handle_midi_msg(t, mc, os, oa).await;
                });
            },
            (),
        )
        .expect("MIDI input port should have allowed a connection");
    let (lock, cvar) = &*running;
    cvar.wait_until(lock.lock().await, |running| !*running)
        .await;
    info!("MIDI listener stopped.")
}

async fn run_osc_listener(
    running: Arc<(Mutex<bool>, Condvar)>,
    mut osc_in_sock: OscSocket,
    output: Sender<Vec<u8>>,
) {
    let (lock, cvar) = &*running;
    let mut run = *lock.lock().await;
    while run {
        async_std::prelude::FutureExt::race(
            async {
                run = *cvar.wait_until(lock.lock().await, |r| !*r).await;
            },
            async {
                match osc_in_sock.next().await {
                    Some(Ok((packet, _peer_addr))) => {
                        task::spawn(handle_osc_pkt(packet, output.clone()));
                    }
                    None => {
                        warn!("OSC input socket was closed.");
                    }
                    Some(stuff) => {
                        warn!("Unrecognized OSC input: {stuff:?}");
                    }
                };
            },
        )
        .await;
    }
    info!("OSC listener stopped.");
}

async fn run_midi_sender(
    input: channel::Receiver<Vec<u8>>,
    midi_output: MidiOutput,
    midi_output_port: MidiOutputPort,
) {
    let mut midi_output_cxn = midi_output
        .connect(&midi_output_port, &format!("{PGM} sender"))
        .expect("Failed to open MIDI output connection.");
    while let Ok(midi) = input.recv().await {
        midi_output_cxn
            .send(&midi)
            .or_else(|e| {
                error!("MIDI port send failed on {} bytes.", midi.len());
                Err(e)
            })
            .ok();
    }
    info!("MIDI sender stopped.")
}

async fn handle_osc_pkt(pkt: OscPacket, out: Sender<Vec<u8>>) {
    let mut midi: Vec<u8> = Vec::new();
    osc_pkt_to_midi(&pkt, &mut midi);
    if midi.is_empty() {
        info!("Ignored OSC packet:\n {pkt:#?}")
    } else {
        let len = midi.len();
        info!("Translated OSC packet to {len} bytes of MIDI:\n{pkt:#?}");
        out.send(midi)
            .await
            .or_else(|e| {
                error!("MIDI channel send failed on {len} bytes: {e:?}");
                Err(e)
            })
            .ok();
    }
}

async fn handle_midi_msg(
    timestamp: u64,
    m: Vec<u8>,
    osc_sender: OscSender,
    osc_out_addrs: Arc<Vec<SocketAddr>>,
) {
    let midi_msg = MidiMsg::from_midi(&m);
    let osc_pkt = match &midi_msg {
        Ok((m, _len)) => midi_to_osc(m),
        Err(e) => {
            error!("{e:?}");
            None
        }
    };
    if osc_pkt.is_some() {
        info!("Parsed and translated this MIDI msg:\n{midi_msg:#?}");
        let pkt = osc_pkt.unwrap();
        info!("Sending this OSC packet:\n {pkt:#?}");
        for a in &*osc_out_addrs {
            if let Err(e) = osc_sender.send_to(pkt.clone(), a).await {
                error!("{}", e);
            };
        }
    }
}

fn midi_to_osc(m: &MidiMsg) -> Option<OscPacket> {
    match m {
        MidiMsg::ChannelVoice {
            channel: Channel::Ch1,
            msg:
                ChannelVoiceMsg::ControlChange {
                    control: ControlChange::TogglePortamento(val),
                },
        } => Some(OscPacket::Message(OscMessage {
            addr: "/key/1".to_string(),
            args: [OscType::Int(if *val { 1 } else { 0 })].to_vec(),
        })),
        _ => None,
    }
}

fn osc_pkt_to_midi(op: &OscPacket, out: &mut Vec<u8>) {
    match op {
        OscPacket::Message(m) => osc_msg_to_midi(m, out),
        OscPacket::Bundle(b) => {
            for p in &b.content {
                osc_pkt_to_midi(p, out);
            }
        }
    }
}
fn osc_msg_to_midi(om: &OscMessage, out: &mut Vec<u8>) {
    let test_osc = OscAddress::new("/key/1".to_string()).unwrap();
    let matcher = Matcher::new(&om.addr);
    if matcher.is_err() {
        error!(
            "Failed to create OSC matcher for incoming address: {}",
            &om.addr
        );
        return;
    }
    let matcher = matcher.unwrap();
    if matcher.match_address(&test_osc) {
        let state = match om.args[0] {
            OscType::Float(v) => {
                if v == 0.0 {
                    Some(false)
                } else if v == 1.0 {
                    Some(true)
                } else {
                    None
                }
            }
            //| OscType::Float(v) | OscType::Long(v) | OscType::Double(v) =>
            //match v {0 => Some(false), 1 => Some(true) },
            OscType::Bool(v) => Some(v),
            _ => None,
        };
        if state.is_none() {
            error!("Unable to decode OSC arg: {om:#?}");
        } else {
            let midi_msg = MidiMsg::ChannelVoice {
                channel: Channel::Ch1,
                msg: ChannelVoiceMsg::ControlChange {
                    control: ControlChange::TogglePortamento(state.unwrap()),
                },
            };
            midi_msg.extend_midi(out);
        }
    }
}
