//! `Translator` implementations for Control Change MIDI.

use std::ops::RangeBounds;

use log::debug;

use super::*;

fn u16_from_cc(cc: &ControlChange) -> Option<u16> {
    match cc {
        ControlChange::BankSelect(v)
        | ControlChange::ModWheel(v)
        | ControlChange::Breath(v)
        | ControlChange::Foot(v)
        | ControlChange::Portamento(v)
        | ControlChange::Volume(v)
        | ControlChange::Balance(v)
        | ControlChange::Pan(v)
        | ControlChange::Expression(v)
        | ControlChange::Effect1(v)
        | ControlChange::Effect2(v)
        | ControlChange::GeneralPurpose1(v)
        | ControlChange::GeneralPurpose2(v)
        | ControlChange::GeneralPurpose3(v)
        | ControlChange::GeneralPurpose4(v)
        | ControlChange::DataEntry(v) => Some(*v),
        ControlChange::Parameter(param) => match param {
            Parameter::Unregistered(v)
            | Parameter::AzimuthAngle3DSoundEntry(v)
            | Parameter::ElevationAngle3DSoundEntry(v)
            | Parameter::Gain3DSoundEntry(v)
            | Parameter::DistanceRatio3DSoundEntry(v)
            | Parameter::MaxiumumDistance3DSoundEntry(v)
            | Parameter::GainAtMaxiumumDistance3DSoundEntry(v)
            | Parameter::ReferenceDistanceRatio3DSoundEntry(v)
            | Parameter::PanSpreadAngle3DSoundEntry(v)
            | Parameter::RollAngle3DSoundEntry(v) => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

fn u8_from_cc(cc: &ControlChange) -> Option<u8> {
    match cc {
        ControlChange::GeneralPurpose5(v)
        | ControlChange::GeneralPurpose6(v)
        | ControlChange::GeneralPurpose7(v)
        | ControlChange::GeneralPurpose8(v)
        | ControlChange::Hold(v)
        | ControlChange::Hold2(v)
        | ControlChange::Sostenuto(v)
        | ControlChange::SoftPedal(v)
        | ControlChange::SoundVariation(v)
        | ControlChange::Timbre(v)
        | ControlChange::ReleaseTime(v)
        | ControlChange::AttackTime(v)
        | ControlChange::Brightness(v)
        | ControlChange::DecayTime(v)
        | ControlChange::VibratoRate(v)
        | ControlChange::VibratoDepth(v)
        | ControlChange::VibratoDelay(v)
        | ControlChange::SoundControl1(v)
        | ControlChange::SoundControl2(v)
        | ControlChange::SoundControl3(v)
        | ControlChange::SoundControl4(v)
        | ControlChange::SoundControl5(v)
        | ControlChange::SoundControl6(v)
        | ControlChange::SoundControl7(v)
        | ControlChange::SoundControl8(v)
        | ControlChange::SoundControl9(v)
        | ControlChange::SoundControl10(v)
        | ControlChange::HighResVelocity(v)
        | ControlChange::PortamentoControl(v)
        | ControlChange::Effects1Depth(v)
        | ControlChange::Effects2Depth(v)
        | ControlChange::Effects3Depth(v)
        | ControlChange::Effects4Depth(v)
        | ControlChange::Effects5Depth(v)
        | ControlChange::ReverbSendLevel(v)
        | ControlChange::TremoloDepth(v)
        | ControlChange::ChorusSendLevel(v)
        | ControlChange::CelesteDepth(v)
        | ControlChange::PhaserDepth(v)
        | ControlChange::DataIncrement(v)
        | ControlChange::DataDecrement(v) => Some(*v),
        ControlChange::Parameter(param) => match param {
            Parameter::TuningProgramSelectEntry(v)
            | Parameter::TuningBankSelectEntry(v)
            | Parameter::PolyphonicExpressionEntry(v) => Some(*v),
            _ => None,
        },
        _ => None,
    }
}

fn bool_from_cc(cc: &ControlChange) -> Option<bool> {
    match cc {
        ControlChange::TogglePortamento(b) | ControlChange::ToggleLegato(b) => Some(*b),
        _ => None,
    }
}

fn float_from_osc(args: &[OscType]) -> Option<f32> {
    let nargs = args.len();
    if nargs != 1 {
        debug!("unexpected # of OSC args: {nargs}");
        return None;
    }
    match args[0] {
        OscType::Float(f) => Some(f),
        _ => None,
    }
}

type CCMatcher = dyn Fn(&ControlChange) -> bool;
type CCMaker = dyn Fn(u16) -> ControlChange;

pub struct ControlChangeHighResRangeTranslator {
    channel: Channel,
    midi_matcher: Box<CCMatcher>,
    midi_maker: Box<CCMaker>,
    low: u16,
    high: u16,
    address: OscAddress,
}

impl ControlChangeHighResRangeTranslator {
    pub fn new(
        channel: Channel,
        midi_matcher: Box<CCMatcher>,
        midi_maker: Box<CCMaker>,
        range: impl RangeBounds<u16>,
        address: &str,
    ) -> Result<Box<dyn Translator>> {
        let low = range.start_bound();
        let low = match low {
            std::ops::Bound::Included(v) => *v,
            std::ops::Bound::Excluded(v) => *v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let high = range.end_bound();
        let high = match high {
            std::ops::Bound::Included(v) => *v,
            std::ops::Bound::Excluded(v) => *v - 1,
            std::ops::Bound::Unbounded => 2u16 ^ 14 - 1,
        };
        let address = OscAddress::new(address.to_string())?;
        Ok(Box::new(Self {
            channel,
            midi_matcher,
            midi_maker,
            low,
            high,
            address,
        }))
    }
}

impl Translator for ControlChangeHighResRangeTranslator {
    fn midi_to_osc(&self, midi: &MidiMsg) -> Option<OscPacket> {
        if let MidiMsg::ChannelVoice {
            channel,
            msg: ChannelVoiceMsg::ControlChange { control },
        } = midi
        {
            if self.channel == *channel && (self.midi_matcher)(control) {
                match u16_from_cc(control) {
                    Some(value) => {
                        return Some(OscPacket::Message(OscMessage {
                            addr: self.address.to_string(),
                            args: vec![OscType::Float(cv_to_normalized_float(
                                value, self.low, self.high,
                            ))],
                        }));
                    }
                    None => {}
                };
            }
        }
        None
    }

    fn osc_to_midi(&self, addr_matcher: &Matcher, args: &[OscType]) -> Option<MidiMsg> {
        if addr_matcher.match_address(&self.address) {
            match float_from_osc(args) {
                Some(value) => {
                    let value = normalized_float_to_cv(value, self.low, self.high);
                    return Some(MidiMsg::ChannelVoice {
                        channel: self.channel,
                        msg: ChannelVoiceMsg::ControlChange {
                            control: (self.midi_maker)(value),
                        },
                    });
                }
                None => {}
            };
        }
        None
    }
}

pub struct ControlChangeBoolTranslator {
    channel: Channel,
    midi_matcher: Box<CCMatcher>,
    midi_maker: Box<CCMaker>,
    off: u16,
    on: u16,
    address: OscAddress,
}

impl ControlChangeBoolTranslator {
    pub fn new(
        channel: Channel,
        midi_matcher: Box<CCMatcher>,
        midi_maker: Box<CCMaker>,
        range: impl RangeBounds<u16>,
        address: &str,
    ) -> Result<Box<dyn Translator>> {
        let off = range.start_bound();
        let off = match off {
            std::ops::Bound::Included(v) => *v,
            std::ops::Bound::Excluded(v) => *v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let on = range.end_bound();
        let on = match on {
            std::ops::Bound::Included(v) => *v,
            std::ops::Bound::Excluded(v) => *v - 1,
            std::ops::Bound::Unbounded => 2u16 ^ 14 - 1,
        };
        let address = OscAddress::new(address.to_string())?;
        Ok(Box::new(Self {
            channel,
            midi_matcher,
            midi_maker,
            off,
            on,
            address,
        }))
    }

    fn cv_to_bool(&self, cv: u16) -> bool {
        let mid = (self.on - self.off) / 2;
        if self.off < self.on {
            cv > mid
        } else {
            cv <= mid
        }
    }
    fn float_to_cv(&self, f: f32) -> u16 {
        if f < 0.5 {
            self.off
        } else {
            self.on
        }
    }
}

impl Translator for ControlChangeBoolTranslator {
    fn midi_to_osc(&self, midi: &MidiMsg) -> Option<OscPacket> {
        if let MidiMsg::ChannelVoice {
            channel,
            msg: ChannelVoiceMsg::ControlChange { control },
        } = midi
        {
            if self.channel == *channel && (self.midi_matcher)(control) {
                let value = bool_from_cc(control)
                .or_else(|| u8_from_cc(control).map(|v| self.cv_to_bool(v as u16)))
                .or_else(|| u16_from_cc(control).map(|v| self.cv_to_bool(v)));
                match value {
                    Some(b) => {
                        return Some(OscPacket::Message(OscMessage {
                            addr: self.address.to_string(),
                            args: vec![OscType::Float(if b {1.0} else {0.0})],
                        }))
                    }
                    None => {}
                }
            }
        }
        None
    }

    fn osc_to_midi(&self, addr_matcher: &Matcher, args: &[OscType]) -> Option<MidiMsg> {
        if addr_matcher.match_address(&self.address) {
            match float_from_osc(args) {
                Some(value) => {
                    let value = self.float_to_cv(value);
                    return Some(MidiMsg::ChannelVoice {
                        channel: self.channel,
                        msg: ChannelVoiceMsg::ControlChange {
                            control: (self.midi_maker)(value),
                        },
                    });
                }
                None => {}
            };
        }
        None
    }
}
