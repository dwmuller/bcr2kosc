use async_osc::{OscMessage, OscPacket, OscType};
use log::{error, info};
use midi_control::*;
use rosc::address::{Matcher, OscAddress};

pub fn midi_to_osc(m: &[u8]) -> Option<OscPacket> {
    let midi_msg = MidiMessage::from(m);

    match midi_msg {
        MidiMessage::ControlChange(
            Channel::Ch1,
            ControlEvent {
                control: 0x41,
                value,
            },
        ) => {
            info!("Translating this MIDI msg: {midi_msg:?}");
            Some(OscPacket::Message(OscMessage {
                addr: "/key/1".to_string(),
                args: [OscType::Int(value.into())].to_vec(),
            }))
        }
        MidiMessage::Invalid => {
            error!("Unparsable MIDI input, {} bytes.", m.len());
            None
        }
        _ => {
            info!("Ignored MIDI msg: {midi_msg:?}");
            None
        }
    }
}

//fn cv_to_bool(v: u8) -> bool { if v < 64 {false} else {true}}
fn bool_to_cv(v: bool) -> u8 {
    if v {
        127
    } else {
        0
    }
}

pub fn osc_pkt_to_midi(op: &OscPacket, out: &mut Vec<u8>) {
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
        } else {
            let midi_msg = control_change(Channel::Ch1, 0x41, state.unwrap());
            let mut m = Vec::<u8>::from(midi_msg);
            out.append(&mut m);
        }
    }
}
