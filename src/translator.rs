//! Translation from OSC to MIDI and vice versa.
//!
//! Notes:
//! * OSC 1.0 supports only these data types: Int, Float, String, Blob, and Time.
//! * Reaper expects Float(1.0) for Boolean true, Float(0.0) for false.
//!

use std::iter;

use log::{error, debug};
use midi_msg::*;
use rosc::address::{Matcher, OscAddress};
use rosc::{OscBundle, OscMessage, OscPacket, OscTime, OscType};

mod ccx;
pub use crate::translator::ccx::*;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Specifies a set of translations between OSC and MIDI messages.
pub struct ServerTranslationSet(Vec<Box<dyn Translator>>);

pub type MMIterator = Box<dyn Iterator<Item = MidiMsg>>;

impl ServerTranslationSet {
    /// Create a new ServerTranslationSet from a vector of translators.
    pub fn new(set: Vec<Box<dyn Translator>>) -> ServerTranslationSet {
        ServerTranslationSet(set)
    }

    pub fn get_test_set() -> Result<ServerTranslationSet> {
        let a = [
            ControlChangeHighResRangeTranslator::new(
                Channel::Ch1,
                Box::new(|cc| matches!(cc, ControlChange::ModWheel(_))),
                Box::new(|val| ControlChange::ModWheel(val)),
                0..=127,
                "/encoder/1",
            )?,
            ControlChangeBoolTranslator::new(
                Channel::Ch1,
                Box::new(|cc| matches!(cc, ControlChange::TogglePortamento(_))),
                Box::new(|val| ControlChange::TogglePortamento(val > 63)),
                0..=127,
                "/key/1",
            )?,
        ];
        Ok(Self::new(a.into_iter().collect()))
    }

    /// Translates a MIDI msg to an OSC packet, if there is at least one valid
    /// mapping to an OSC message. The packet may contain multiple messages.
    pub fn midi_msg_to_osc(&self, midi_msg: MidiMsg) -> Option<OscPacket> {
        debug!("Translating MIDI: {midi_msg:?}");
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
            Some(OscPacket::Bundle(OscBundle {
                timetag: OscTime {
                    seconds: 0,
                    fractional: 0,
                },
                content: msgs,
            }))
        }
    }

    pub fn osc_pkt_to_midi(&self, op: &OscPacket) -> MMIterator {
        match op {
            OscPacket::Message(om) => {
                let matcher = Matcher::new(&om.addr);
                if matcher.is_err() {
                    error!(
                        "Failed to create OSC matcher for incoming address: {}",
                        &om.addr
                    );
                    return Box::new(iter::empty());
                }
                let matcher = matcher.unwrap();
                let v: Vec<MidiMsg> = self
                    .0
                    .iter()
                    .filter_map(|x| x.osc_to_midi(&matcher, &om.args))
                    .collect();
                Box::new(v.into_iter())
            }
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
}

pub trait Translator {
    fn midi_to_osc(&self, midi: &MidiMsg) -> Option<OscPacket>;
    fn osc_to_midi(&self, addr_matcher: &Matcher, args: &[OscType]) -> Option<MidiMsg>;
}

//struct NoteOnTranslator(Channel, MidiNote, String);

/// Translate a MIDI control value to a normalized float (0.0 thru 1.0).
fn cv_to_normalized_float(v: u16, low: u16, high: u16) -> f32 {
    (v - low) as f32 / (high - low) as f32
}

/// Translate a normalized float (0.0 thru 1.0) to a MIDI control value.
fn normalized_float_to_cv(v: f32, low: u16, high: u16) -> u16 {
    (v * (high - low) as f32).round() as u16 + low
}
