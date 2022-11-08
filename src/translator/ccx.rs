//! `Translator` implementations for Control Change MIDI.

use super::*;

pub struct ControlChangeRangeTranslator {
    channel: Channel,
    control: u8,
    low: u8,
    high: u8,
    address: String,
}

impl ControlChangeRangeTranslator {
    pub fn new(
        channel: Channel,
        control: u8,
        low: u8,
        high: u8,
        address: &str,
    ) -> Box<dyn Translator> {
        Box::new(Self {
            channel,
            control,
            low,
            high,
            address: address.to_string(),
        })
    }
}

impl Translator for ControlChangeRangeTranslator {
    fn midi_to_osc(&self, midi: &MidiMessage) -> Option<OscPacket> {
        use MidiMessage::*;
        if let ControlChange(ch, ControlEvent { control, value }) = midi {
            if (self.channel == Channel::from(*ch)) && (self.control == *control) {
                return Some(OscPacket::Message(OscMessage {
                    addr: self.address.clone(),
                    args: vec![OscType::Float(cv_to_normalized_float(
                        *value, self.low, self.high,
                    ))],
                }));
            }
        }
        None
    }

    fn osc_to_midi(&self, addr_matcher: Matcher, args: &[OscType]) -> Option<MidiMessage> {
        // TODO: Performance nightmare here:
        if addr_matcher.match_address(&OscAddress::new(self.address.clone()).unwrap()) {
            return Some(MidiMessage::ControlChange(
                self.channel,
                ControlEvent {
                    control: self.control,
                    value: normalized_float_to_cv(
                        OscType::float(args[0].clone()).unwrap(),
                        self.low,
                        self.high,
                    ),
                },
            ));
        }
        None
    }
}

pub struct ControlChangeBoolTranslator {
    channel: Channel,
    control: u8,
    off: u8,
    on: u8,
    address: String,
}

impl ControlChangeBoolTranslator {
    pub fn new(
        channel: Channel,
        control: u8,
        off: u8,
        on: u8,
        address: &str,
    ) -> Box<dyn Translator> {
        Box::new(Self {
            channel,
            control,
            off,
            on,
            address: address.to_string(),
        })
    }

    fn cv_to_float(&self, cv: u8) -> f32 {
        let b = if self.off == cv {
            false
        } else if self.on == cv {
            true
        } else if self.off < self.on {
            let mid = (self.on - self.off) / 2;
            cv > mid
        } else {
            let mid = (self.off - self.on) / 2;
            cv < mid
        };
        if b {
            1.0
        } else {
            0.0
        }
    }
    fn float_to_cv(&self, f: f32) -> u8 {
        if f < 0.5 {
            self.off
        } else {
            self.on
        }
    }
}
impl Translator for ControlChangeBoolTranslator {
    fn midi_to_osc(&self, midi: &MidiMessage) -> Option<OscPacket> {
        use MidiMessage::*;
        if let ControlChange(ch, ControlEvent { control, value }) = midi {
            if (self.channel == Channel::from(*ch)) && (self.control == *control) {
                return Some(OscPacket::Message(OscMessage {
                    addr: self.address.clone(),
                    args: vec![OscType::Float(self.cv_to_float(*value))],
                }));
            }
        }
        None
    }

    fn osc_to_midi(&self, addr_matcher: Matcher, args: &[OscType]) -> Option<MidiMessage> {
        // TODO: Performance nightmare here:
        if addr_matcher.match_address(&OscAddress::new(self.address.clone()).unwrap()) {
            return Some(MidiMessage::ControlChange(
                self.channel,
                ControlEvent {
                    control: self.control,
                    value: self.float_to_cv(OscType::float(args[0].clone()).unwrap()),
                },
            ));
        }
        None
    }
}
