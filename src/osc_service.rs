#![allow(unused_variables)]
//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to a configured UDP port.

use midi_msg::MidiMsg;
use midir::{MidiInput, MidiInputPort};
use std::error::Error;

use crate::midi_util::*;
use crate::PGM;
use async_osc::{OscPacket, OscSocket};
use async_std::net::SocketAddr;
use async_std::stream::StreamExt;
use async_std::sync::{Arc, Condvar, Mutex};
use async_std::task::spawn;
use log::{info};

pub struct BCtlOscSvc {
    pair: Arc<(Mutex<bool>, Condvar)>,
}
impl BCtlOscSvc {
    pub async fn stop(self) {
        let (lock, cvar) = &*self.pair;
        let mut stopping = lock.lock().await;
        *stopping = true;
        cvar.notify_all();
    }
}

pub async fn start(
    midi_in_port_name: &str,
    midi_out_port_name: &str,
    osc_in_addr: &SocketAddr,
    osc_out_addrs: &[SocketAddr],
) -> Result<BCtlOscSvc, Box<dyn Error>> {
    let pair = Arc::new((Mutex::new(false), Condvar::new()));
    {
        let pair = pair.clone();
        let midi_input = MidiInput::new(&format!("{PGM} listening to B-Control"))?;
        let midi_input_port = find_port(&midi_input, midi_in_port_name)?;
        // TODO
        //let osc_out_sock = OscSocket::bind("127.0.0.1:0").await?;
        spawn(async move {
            run_midi_listener(pair, midi_input, midi_input_port).await;
        });
    }
    {
        let pair = pair.clone();
        let osc_in_sock = OscSocket::bind(osc_in_addr).await?;
        // TODO
        //let midi_output = MidiOutput::new(&format!("{PGM} feedback to B-Control"))?;
        //let midi_output_port = find_port(&midi_output, midi_out_port_name)?;
        spawn(async move {
            run_osc_listener(pair, osc_in_sock).await;
        });
    }
    Ok(BCtlOscSvc { pair })
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
                spawn(async move {
                    handle_midi_msg(t, mc).await;
                });
            },
            (),
        )
        .expect("MIDI input port should have allowed a connection");
    let (lock, cvar) = &*pair;
    let mut stopping = lock.lock().await;
    while !*stopping {
        stopping = cvar.wait(stopping).await;
    }
}

async fn run_osc_listener(pair: Arc<(Mutex<bool>, Condvar)>, mut osc_in_sock: OscSocket) {
    let (lock, cvar) = &*pair;
    let mut stopping = lock.lock().await;
    while !*stopping {
        stopping = cvar.wait(stopping).await;
        if let Some(packet) = osc_in_sock.next().await {
            if let Ok((packet, peer_addr)) = packet {
                spawn(handle_osc_pkt(packet, peer_addr));
            }
        }
    }
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
