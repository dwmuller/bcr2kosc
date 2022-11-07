//! Translation from OSC to MIDI and vice versa.
//!
//! Notes:
//! * OSC 1.0 supports only these data types: Int, Float, String, Blob, and Time.
//! * Reaper expects Float(1.0) for Boolean true, Float(0.0) for false.
//!

use std::iter;

use log::{error};
use midi_control::*;
use rosc::address::{Matcher, OscAddress};
use rosc::{OscMessage, OscPacket, OscType, OscBundle, OscTime};

mod ccx;
pub use crate::translator::ccx::*;

/// Specifies a set of translations between OSC and MIDI messages.
pub struct ServerTranslationSet(Vec<Box<dyn Translator>>);

pub type MMIterator = Box<dyn Iterator<Item = MidiMessage>>;

impl ServerTranslationSet {
    /// Create a new ServerTranslationSet from a vector of translators.
    pub fn new(set: Vec<Box<dyn Translator>>) -> ServerTranslationSet {
        ServerTranslationSet(set)
    }

    pub fn get_test_set() -> ServerTranslationSet {
        Self::new(vec![
            ControlChangeRangeTranslator::new(Channel::Ch1, 1, 0, 127, "/encoder/1"),
            ControlChangeBoolTranslator::new(Channel::Ch1, 0x41, 0, 127, "/key/1"),
        ])
    }

    /// Translates a MIDI msg to an OSC packet, if there is at least one valid
    /// mapping to an OSC message. The packet may contain multiple messages.
    pub fn midi_msg_to_osc(&self, midi_msg: MidiMessage) -> Option<OscPacket> {
        let msgs: Vec<OscPacket> = self
            .0
            .iter()
            .map(|x| x.midi_to_osc(&midi_msg))
            .filter_map(|i| i)
            .collect();
        if msgs.is_empty() {
            None
        } else if msgs.len() == 1 {
            Some(msgs.into_iter().last().unwrap())
        } else {
            Some(OscPacket::Bundle(OscBundle { timetag: OscTime{ seconds: 0, fractional: 0 }, content: msgs}))
        }
    }

    pub fn osc_pkt_to_midi(&self, op: &OscPacket) -> MMIterator {
        match op {
            OscPacket::Message(m) => self.osc_msg_to_midi(m),
            OscPacket::Bundle(b) => {
                let sub = b
                    .content
                    .iter()
                    .map(|p| self.osc_pkt_to_midi(p))
                    .collect::<Vec<MMIterator>>();
                Box::new(sub.into_iter().flatten())
            }
        }
    }
    fn osc_msg_to_midi(&self, om: &OscMessage) -> Box<dyn Iterator<Item = MidiMessage> + Send> {
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
}

pub trait Translator {
    fn midi_to_osc(&self, midi: &MidiMessage) -> Option<OscPacket>;
    fn osc_to_midi(&self, addr_matcher: Matcher, args: &[OscType]) -> Option<MidiMessage>;
}

//struct NoteOnTranslator(Channel, MidiNote, String);

/// Translate a MIDI control value to a normalized float (0.0 thru 1.0).
fn cv_to_normalized_float(v: u8, low: u8, high: u8) -> f32 {
    (v - low) as f32 / (high - low) as f32
}

/// Translate a normalized float (0.0 thru 1.0) to a MIDI control value.
fn normalized_float_to_cv(v: f32, low: u8, high: u8) -> u8 {
    (v * (high - low) as f32).round() as u8 + low
}

/// Translate a Boolean to a control value.
fn bool_to_cv(v: bool) -> u8 {
    if v {
        127
    } else {
        0
    }
}
