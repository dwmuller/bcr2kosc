#![allow(unused_variables)]
//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to a configured UDP port.

use async_osc::{OscPacket, OscSocket};
use async_std::net::SocketAddr;
use async_std::stream::StreamExt;
use async_std::sync::{Arc, Condvar, Mutex};
use async_std::task::{self, JoinHandle};

use log::{info, warn};
use midi_msg::MidiMsg;
use midir::{MidiInput, MidiInputPort};
use std::error::Error;

use crate::midi_util::*;
use crate::PGM;

pub struct BCtlOscSvc {
    pub midi_in_port_name: String,
    pub midi_out_port_name: String,
    pub osc_in_addr: SocketAddr,
    pub osc_out_addrs: Vec<SocketAddr>,

    stop_sentinel: Arc<(Mutex<bool>, Condvar)>,
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
            osc_out_addrs: osc_out_addrs.to_vec(),
            stop_sentinel: Arc::new((Mutex::new(false), Condvar::new())),
            spawned_tasks: Vec::new(),
        }
    }

    pub async fn stop(mut self) {
        let (lock, cvar) = &*self.stop_sentinel;
        {
            let mut stopping = lock.lock().await;
            *stopping = true;
            cvar.notify_all();
        }
        for ele in self.spawned_tasks.drain(0..) {
            ele.await;
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        {
            let pair = self.stop_sentinel.clone();
            let midi_input = MidiInput::new(&format!("{PGM} listening to B-Control"))?;
            let midi_input_port = find_port(&midi_input, &self.midi_in_port_name)?;
            // TODO
            //let osc_out_sock = OscSocket::bind("127.0.0.1:0").await?;
            self.spawned_tasks.push(
                task::Builder::new()
                    .name("MIDI listener task".to_string())
                    .spawn(async move {
                        Self::run_midi_listener(pair, midi_input, midi_input_port).await;
                    })
                    .unwrap(),
            );
        }
        {
            let pair = self.stop_sentinel.clone();
            let osc_in_sock = OscSocket::bind(self.osc_in_addr).await?;
            // TODO
            //let midi_output = MidiOutput::new(&format!("{PGM} feedback to B-Control"))?;
            //let midi_output_port = find_port(&midi_output, self.midi_out_port_name)?;
            self.spawned_tasks.push(
                task::Builder::new()
                    .name("OSC listener task".to_string())
                    .spawn(async move {
                        Self::run_osc_listener(pair, osc_in_sock).await;
                    })
                    .unwrap(),
            );
        }
        Ok(())
    }

    async fn run_midi_listener(
        pair: Arc<(Mutex<bool>, Condvar)>,
        midi_input: MidiInput,
        midi_input_port: MidiInputPort,
    ) {
        let midi_input_cxn = midi_input
            .connect(
                &midi_input_port,
                "{PGM} listener",
                move |t, m, _| {
                    let mc = m.to_vec();
                    task::spawn(async move {
                        Self::handle_midi_msg(t, mc).await;
                    });
                },
                (),
            )
            .expect("MIDI input port should have allowed a connection");
        let (lock, cvar) = &*pair;
        cvar.wait_until(lock.lock().await, |stopping| *stopping)
            .await;
        info!("MIDI listener stopped.")
    }

    async fn run_osc_listener(pair: Arc<(Mutex<bool>, Condvar)>, mut osc_in_sock: OscSocket) {
        let (lock, cvar) = &*pair;
        loop {
            let stop = async_std::prelude::FutureExt::race(
                async { *cvar.wait_until(lock.lock().await, |stop| *stop).await },
                async {
                    match osc_in_sock.next().await {
                        Some(Ok((packet, peer_addr))) => {
                            task::spawn(Self::handle_osc_pkt(packet, peer_addr));
                            false
                        }
                        None => {
                            warn!("OSC input socket was closed.");
                            true
                        }
                        Some(stuff) => {
                            warn!("Unrecognized OSC input: {stuff:?}");
                            false
                        }
                    }
                },
            )
            .await;
            if stop {
                break;
            }
        }
        info!("OSC listener stopped.");
    }

    async fn handle_osc_pkt(pkt: OscPacket, sender: SocketAddr) {
        info!("{pkt:?}"); // process OSC packet
    }

    async fn handle_midi_msg(timestamp: u64, m: Vec<u8>) {
        let midi_msg = MidiMsg::from_midi(&m);
        info!("{midi_msg:?}");
        // TODO:
        // - Figure out if the msg is relevant.
        // - Dispatch to all targeted OSC services.
    }
}
