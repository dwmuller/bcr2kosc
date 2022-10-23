#![allow(unused_variables)]
//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to a configured UDP port.

use async_osc::{OscMessage, OscPacket, OscSender, OscSocket, OscType};
use async_std::net::SocketAddr;
use async_std::stream::StreamExt;
use async_std::sync::{Arc, Condvar, Mutex};
use async_std::task::{self, spawn, JoinHandle};

use log::{error, info, warn};
use midi_msg::{Channel, ChannelVoiceMsg, ControlChange, MidiMsg};
use midir::{MidiInput, MidiInputPort};
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
            // TODO
            let osc_out_sock = OscSocket::bind("127.0.0.1:0").await?;
            self.spawned_tasks.push(spawn(async move {
                Self::run_midi_listener(
                    running,
                    midi_input,
                    midi_input_port,
                    osc_out_sock,
                    osc_out_addrs,
                )
                .await;
            }));
        }
        {
            let running = self.running.clone();
            let osc_in_sock = OscSocket::bind(self.osc_in_addr).await?;
            // TODO
            //let midi_output = MidiOutput::new(&format!("{PGM} feedback to B-Control"))?;
            //let midi_output_port = find_port(&midi_output, self.midi_out_port_name)?;
            self.spawned_tasks.push(spawn(async move {
                Self::run_osc_listener(running, osc_in_sock).await;
            }));
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
                "{PGM} listener",
                move |t, m, _| {
                    let mc = m.to_vec();
                    let os = osc_out_sock.sender();
                    let oa = osc_out_addrs.clone();
                    task::spawn(async move {
                        Self::handle_midi_msg(t, mc, os, oa).await;
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

    async fn run_osc_listener(running: Arc<(Mutex<bool>, Condvar)>, mut osc_in_sock: OscSocket) {
        let (lock, cvar) = &*running;
        let mut run = *lock.lock().await;
        while run {
            async_std::prelude::FutureExt::race(
                async {
                    run = *cvar.wait_until(lock.lock().await, |r| !*r).await;
                },
                async {
                    match osc_in_sock.next().await {
                        Some(Ok((packet, peer_addr))) => {
                            task::spawn(Self::handle_osc_pkt(packet, peer_addr));
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

    async fn handle_osc_pkt(pkt: OscPacket, sender: SocketAddr) {
        info!("{pkt:?}"); // process OSC packet
    }

    async fn handle_midi_msg(
        timestamp: u64,
        m: Vec<u8>,
        osc_sender: OscSender,
        osc_out_addrs: Arc<Vec<SocketAddr>>,
    ) {
        let midi_msg = MidiMsg::from_midi(&m);
        info!("{midi_msg:?}");
        // TODO:
        // - Figure out if the msg is relevant.
        // - Dispatch to all targeted OSC services.
        let osc_msg = match midi_msg {
            Ok((
                MidiMsg::ChannelVoice {
                    channel: Channel::Ch1,
                    msg:
                        ChannelVoiceMsg::ControlChange {
                            control: ControlChange::TogglePortamento(val),
                        },
                },
                len,
            )) => Some(OscMessage {
                addr: "/key/1".to_string(),
                args: [OscType::Int(if val { 1 } else { 0 })].to_vec(),
            }),
            Ok(_) => None,
            Err(e) => {
                error!("{}", e);
                None
            }
        };
        if osc_msg.is_some() {
            info!("{:?}", osc_msg);
            let osc_message = osc_msg.unwrap();
            let pkt = OscPacket::Message(osc_message);
            for a in &*osc_out_addrs {
                if let Err(e) = osc_sender.send_to(pkt.clone(), a).await {
                    error!("{}", e);
                };
            }
        }
    }
}
