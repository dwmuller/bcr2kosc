//! Behringer's MIDI System Exclusive syntax for B-Controls
//!
//! Types to represent system exclusive messages for Behringer's
//! BCR2000 and BCF2000 MIDI controllers, and methods to translate them to and
//! from `midi_control::MidiMessage::SysEx` enum variants.
//! 
//! The `io` sub-module, which is re-exported here, contains functions for
//! requesting and receiving specific types of data from a B-Control when given
//! a `Stream` and `Sink` of `MidiMessage` objects. See `midi-io`.
//!
//! This is based on the amazing reverse engineering work by Mark van den
//! Berg, published on https://mountainutilities.eu/. It follows patterns
//! used in the midi_msg library crate and uses some types from it.
//!

// TODO: Review overhead introduced by using MidiMessage. Consider bypassing.

use std::{error::Error, fmt::Display};

use midi_control::{message::SysExType, sysex::ManufacturerId, MidiMessage, SysExEvent};

mod io;
pub use io::*;

/// Behringer's MIDI manufacturer ID.
pub const BEHRINGER: ManufacturerId = ManufacturerId::ExtId(0x20u8, 0x32u8);

/// B-Control mode system exclusive data. All system exclusive message data
/// to or from the BC devices have this structure.
pub struct BControlSysEx {
    pub device: DeviceID,
    pub model: BControlModel,
    pub command: BControlCommand,
}

/// BControl device number. Each controller can be set to answer queries
/// addressed to a specific device number, 0 through 15. In the controller's LCD
/// display or in UIs, the numbers are usually shown as 1 through 16.
#[derive(Debug, PartialEq, Eq)]
pub enum DeviceID {
    /// BC device number, zero through 15.
    Device(u8),
    /// BC device number 0x7f, denoting "any".
    Any,
}

impl DeviceID {
    pub fn match_device(&self, device: u8) -> bool {
        match self {
            DeviceID::Device(d) => &device == d,
            DeviceID::Any => true,
        }
    }
}

type ParseError = Box<dyn Error>;
fn error<T>(s: &str) -> Result<T, ParseError> {
    Err(ParseError::from(s))
}

impl BControlSysEx {
    pub fn to_midi(&self) -> Vec<u8> {
        let mut r: Vec<u8> = vec![];
        self.extend_midi(&mut r);
        r
    }
    pub fn extend_midi(&self, v: &mut Vec<u8>) {
        v.push(match self.device {
            DeviceID::Device(d) => d.min(15),
            DeviceID::Any => 0x7f,
        });
        v.push(match self.model {
            BControlModel::BCR => 0x15,
            BControlModel::BCF => 0x14,
            BControlModel::Any => 0x7f,
        });
        self.command.extend_midi(v);
        v.push(midi_control::consts::EOX);
    }
    pub fn from_midi(m: &[u8]) -> Result<(Self, usize), ParseError> {
        if m.len() == 0 {
            return error("no sysex data");
        }
        // Elide EOX byte if present. Some MIDI parser packages do this already,
        // some do not.
        let mut m = m;
        if m[m.len() - 1] == midi_control::consts::EOX {
            m = &m[..m.len() - 1];
        };
        if m.len() >= 3 {
            let device = match m[0] {
                0..=15 => DeviceID::Device(m[0]),
                0x7f => DeviceID::Any,
                n => return error(&format!("invalid device id. ({n})")),
            };
            let model = match m[1] {
                0x14 => BControlModel::BCF,
                0x15 => BControlModel::BCR,
                0x7f => BControlModel::Any,
                n => return error(&format!("bad B-Control model number ({n:x})")),
            };
            let (command, used) = match m[2] {
                0x01 => (BControlCommand::RequestIdentity, 0),
                0x02 => (
                    BControlCommand::SendIdentity {
                        id_string: string_from_midi(&m[3..])?,
                    },
                    m.len() - 3,
                ),
                0x20 => (
                    BControlCommand::SendBclMessage {
                        msg_index: u14_from_midi_msb_lsb(&m[3..])?,
                        text: string_from_midi(&m[5..])?,
                    },
                    m.len() - 3,
                ),
                0x21 => {
                    if m.len() > 6 {
                        // Supposedly the preset name will be exactly 26 chars.
                        (
                            BControlCommand::SendPresetName {
                                preset: PresetIndex::from_midi(&m[3..])?,
                                name: string_from_midi(&m[4..])?,
                            },
                            m.len() - 3,
                        )
                    } else {
                        (
                            BControlCommand::BclReply {
                                msg_index: u14_from_midi_msb_lsb(&m[3..])?,
                                error_code: u8_from_midi(&m[5..])?,
                            },
                            3,
                        )
                    }
                }
                0x22 => (BControlCommand::SelectPreset { index: m[3] }, 1),
                0x34 => (
                    BControlCommand::SendFirmware {
                        data: m[3..].to_vec(),
                    },
                    m.len() - 3,
                ),
                0x35 => (
                    BControlCommand::FirmwareReply {
                        mem_addr: u14_from_midi_msb_lsb(&m[3..])?,
                        err: u8_from_midi(&m[5..])?,
                    },
                    3,
                ),
                0x40 => (
                    BControlCommand::RequestData(PresetIndex::from_midi(&m[3..])?),
                    1,
                ),
                0x41 => (BControlCommand::RequestGlobalSetup, 0),
                0x42 => (
                    BControlCommand::RequestPresetName {
                        preset: PresetIndex::from_midi(&m[3..])?,
                    },
                    1,
                ),
                0x43 => (BControlCommand::RequestSnapshot, 0),
                0x78 => (BControlCommand::SendText, 0),
                cmd => return error(&format!("invalid B-Control command {cmd:x}")),
            };
            let result = BControlSysEx {
                device,
                model,
                command,
            };
            Ok((result, used + 3))
        } else {
            error("unexpected end")
        }
    }
}

impl From<&BControlSysEx> for Vec<u8> {
    fn from(b: &BControlSysEx) -> Self {
        b.to_midi()
    }
}
impl From<&BControlSysEx> for MidiMessage {
    fn from(bc: &BControlSysEx) -> Self {
        let bdata = bc.to_midi();
        let req = MidiMessage::SysEx(SysExEvent {
            r#type: SysExType::Manufacturer(BEHRINGER),
            data: bdata,
        });
        req
    }
}

impl TryFrom<&MidiMessage> for BControlSysEx {
    type Error = ParseError;

    fn try_from<'a>(value: &MidiMessage) -> Result<Self, Self::Error> {
        if let MidiMessage::SysEx(SysExEvent {
            r#type: SysExType::Manufacturer(BEHRINGER),
            data,
        }) = value
        {
            // Recognized as a Behringer sysex. Parse the sysex payload.
            match BControlSysEx::from_midi(&data) {
                Ok(bcse) => Ok(bcse.0),
                Err(e) => Err(e),
            }
        } else {
            error("not a Behringer sysex")
        }
    }
}

impl TryFrom<&[u8]> for BControlSysEx {
    type Error = ParseError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        match Self::from_midi(value) {
            Ok(value) => Ok(value.0),
            Err(e) => Err(e),
        }
    }
}

/// Specifies the B-Control device models addressed by a B-Control request, or
/// responding to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BControlModel {
    BCR,
    BCF,
    Any,
}

impl Display for BControlModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BControlModel::BCR => "BCR",
            BControlModel::BCF => "BCF",
            BControlModel::Any => "?",
        }
        .fmt(f)
    }
}

/// B-Control command data appears in system exclusive messages sent to or
/// recieved from  B-Control devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BControlCommand {
    ///
    SendBclMessage {
        msg_index: u16,
        text: String,
    },

    RequestIdentity,
    SelectPreset {
        index: u8,
    },
    SendFirmware {
        data: Vec<u8>,
    },
    RequestData(PresetIndex),
    RequestGlobalSetup,
    RequestPresetName {
        preset: PresetIndex,
    },
    RequestSnapshot,

    SendIdentity {
        id_string: String,
    },
    BclReply {
        msg_index: u16,
        error_code: u8,
    },
    SendPresetName {
        preset: PresetIndex,
        name: String,
    },
    FirmwareReply {
        mem_addr: u16,
        err: u8,
    },
    SendText,
}
impl BControlCommand {
    pub fn extend_midi(&self, v: &mut Vec<u8>) {
        match self {
            BControlCommand::RequestIdentity => {
                v.push(0x01);
            }
            BControlCommand::SendBclMessage { msg_index, text } => {
                v.push(0x02);
                u14_to_midi_msb_lsb(*msg_index, v);
                extend_midi_from_string(text, v);
            }
            BControlCommand::SelectPreset { index } => {
                v.push(0x22);
                v.push(*index);
            }
            BControlCommand::SendFirmware { data } => {
                v.push(0x34);
                data.iter().for_each(|b| v.push(*b));
            }
            BControlCommand::RequestData(preset) => {
                v.push(0x40);
                preset.extend_midi(v);
            }
            BControlCommand::RequestGlobalSetup => {
                v.push(0x41);
            }
            BControlCommand::RequestPresetName { preset } => {
                v.push(0x42);
                preset.extend_midi(v);
            }
            BControlCommand::RequestSnapshot => {
                v.push(0x43);
            }
            BControlCommand::SendIdentity { id_string } => {
                v.push(0x02);
                extend_midi_from_string(id_string, v);
            }
            BControlCommand::BclReply {
                msg_index,
                error_code,
            } => {
                v.push(0x21);
                u14_to_midi_msb_lsb(*msg_index, v);
                v.push(*error_code);
            }
            BControlCommand::SendPresetName { preset, name } => {
                v.push(0x21);
                v.push(0);
                preset.extend_midi(v);
                extend_midi_from_string(name, v);
            }
            BControlCommand::FirmwareReply { mem_addr, err } => {
                v.push(0x35);
                u14_to_midi_msb_lsb(*mem_addr, v);
                v.push(*err);
            }
            BControlCommand::SendText => {
                v.push(0x78);
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PresetIndex {
    Preset(u8),
    All,
    Temporary,
}

impl PresetIndex {
    fn from_midi(m: &[u8]) -> Result<PresetIndex, ParseError> {
        match u8_from_midi(m)? {
            0..=31 => Ok(PresetIndex::Preset(m[0])),
            0x7e => Ok(PresetIndex::All),
            0x7f => Ok(PresetIndex::Temporary),
            n => error(&format!("bad preset index ({n})")),
        }
    }
    fn extend_midi(self, v: &mut Vec<u8>) {
        match self {
            PresetIndex::Preset(index) => v.push(index),
            PresetIndex::All => v.push(0x7e),
            PresetIndex::Temporary => v.push(0x7f),
        }
    }
}
impl Display for PresetIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetIndex::Preset(n) => write!(f, "{n}"),
            PresetIndex::All => write!(f, "all"),
            PresetIndex::Temporary => write!(f, "temp"),
        }
    }
}

#[inline]
fn u8_from_midi(m: &[u8]) -> Result<u8, ParseError> {
    if m.is_empty() {
        error("unexpected end")
    } else if m[0] > 127 {
        error("MIDI byte overflow")
    } else {
        Ok(m[0])
    }
}
#[inline]
fn u14_from_midi_msb_lsb(m: &[u8]) -> Result<u16, ParseError> {
    if m.len() < 2 {
        error("unexpected end")
    } else {
        let (msb, lsb) = (m[0], m[1]);
        if lsb > 127 || msb > 127 {
            error("MIDI byte overflow")
        } else {
            let mut x = lsb as u16;
            x += (msb as u16) << 7;
            Ok(x)
        }
    }
}

fn string_from_midi(m: &[u8]) -> Result<String, ParseError> {
    match String::from_utf8(m.to_vec()) {
        Ok(s) => Ok(s),
        Err(e) => error(&format!("invalid string in MIDI, {e:?}")),
    }
}

fn extend_midi_from_string(text: &str, v: &mut Vec<u8>) {
    text.as_bytes().iter().for_each(|c| v.push(*c));
}

fn u14_to_midi_msb_lsb(n: u16, m: &mut Vec<u8>) {
    if n > 16383 {
        panic!("Number too large to represent as two bytes of MIDI data.")
    } else {
        m.push(((n & 0x3f80) >> 7) as u8);
        m.push((n & 0x007f) as u8);
    }
}
