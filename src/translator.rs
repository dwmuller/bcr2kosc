//! OSC 1.0 supports only these data types: Int, Float, String, Blob, and Time.
//! Reaper expects Float(1.0) for Boolean true, Float(0.0) for false.
//!

use std::iter;

use async_stream::stream;
use log::{error, debug};
use midi_control::*;
use rosc::address::{Matcher, OscAddress};
use rosc::{OscMessage, OscPacket, OscType};
use tokio_stream::{Stream, StreamExt};

pub fn midi_to_osc<I: Stream<Item = MidiMessage> + Unpin>(
    midi: I,
) -> impl Stream<Item = OscPacket> {
    midi.filter_map(|midi_msg| {
        match midi_msg {
            MidiMessage::ControlChange(
                Channel::Ch1,
                ControlEvent {
                    control: 0x41,
                    value,
                },
            ) => {
                //info!("Translating this MIDI msg: {midi_msg:?}");

                // Strangely, Reaper wants boolean values as floats, and insists
                // on "1.0" or "0.0".
                let value = cv_to_bool(value);
                Some(OscPacket::Message(OscMessage {
                    addr: "/key/1".to_string(),
                    args: [OscType::Float(if value { 1.0 } else { 0.0 })].to_vec(),
                    //                    args: [OscType::Bool(value)].to_vec(),
                }))
            }
            MidiMessage::Invalid => {
                //error!("Invalid MIDI input.");
                None
            }
            _ => {
                debug!("Ignored MIDI msg: {midi_msg:?}");
                None
            }
        }
    })
}

//fn cv_to_bool(v: u8) -> bool { if v < 64 {false} else {true}}
fn bool_to_cv(v: bool) -> u8 {
    if v {
        127
    } else {
        0
    }
}

fn cv_to_bool(v: u8) -> bool {
    v >= 64
}

pub fn osc_to_midi<I: Stream<Item = OscPacket> + Unpin>(
    mut osc: I,
) -> impl Stream<Item = MidiMessage> {
    stream! {
        while let Some(op) = osc.next().await {
            let i = osc_pkt_to_midi(&op);
            for m in i {yield m};
        }
    }
}

fn osc_pkt_to_midi(op: &OscPacket) -> Box<dyn Iterator<Item = MidiMessage> + Send + '_> {
    match op {
        OscPacket::Message(m) => osc_msg_to_midi(m),
        OscPacket::Bundle(b) => Box::new(b.content.iter().map(|p| osc_pkt_to_midi(p)).flatten()),
    }
}

fn osc_msg_to_midi(om: &OscMessage) -> Box<dyn Iterator<Item = MidiMessage> + Send> {
    let test_osc = OscAddress::new("/key/1".to_string()).unwrap();
    let matcher = Matcher::new(&om.addr);
    if matcher.is_err() {
        error!(
            "Failed to create OSC matcher for incoming address: {}",
            &om.addr
        );
        return Box::new(iter::empty());
    }
    let matcher = matcher.unwrap();
    if matcher.match_address(&test_osc) {
        let state: Option<u8> = match om.args[0] {
            OscType::Float(v) => {
                if v == 0.0 {
                    Some(0)
                } else if v == 1.0 {
                    Some(127)
                } else {
                    None
                }
            }
            //| OscType::Float(v) | OscType::Long(v) | OscType::Double(v) =>
            //match v {0 => Some(false), 1 => Some(true) },
            OscType::Bool(v) => Some(bool_to_cv(v)),
            _ => None,
        };
        if state.is_none() {
            error!("Unable to decode OSC arg: {om:?}");
            Box::new(iter::empty())
        } else {
            Box::new(iter::once(control_change(
                Channel::Ch1,
                0x41,
                state.unwrap(),
            )))
        }
    } else {
        Box::new(iter::empty())
    }
}
