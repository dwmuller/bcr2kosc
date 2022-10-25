use async_osc::{OscMessage, OscPacket, OscType};
use log::{error, info};
use midi_msg::{Channel, ChannelVoiceMsg, ControlChange, MidiMsg};
use rosc::address::{Matcher, OscAddress};

pub fn midi_to_osc(m: &[u8]) -> Option<OscPacket> {
    let midi_msg = MidiMsg::from_midi(&m);

    let midi_msg = match &midi_msg {
        Ok((m, _len)) => m,
        Err(e) => {
            error!("Unparsable MIDI input:\n{e:?}");
            return None;
        }
    };
    match midi_msg {
        MidiMsg::ChannelVoice {
            channel: Channel::Ch1,
            msg:
                ChannelVoiceMsg::ControlChange {
                    control: ControlChange::TogglePortamento(val),
                },
        } => {
            info!("Translating this MIDI msg:\n{midi_msg:#?}");
            Some(OscPacket::Message(OscMessage {
                addr: "/key/1".to_string(),
                args: [OscType::Int(if *val { 1 } else { 0 })].to_vec(),
            }))
        }
        _ => {
            info!("Ignored MIDI msg:\n{midi_msg:#?}");
            None
        }
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
