//! Behringer's MIDI System Exclusive syntax for B-Controls
//!
//! Types to represent system exclusive messages for Behringer's
//! BCR2000 and BCF2000 MIDI controllers.
//!
//! This is based on the amazing reverse engineering work by Mark van den
//! Berg, published on https://mountainutilities.eu/. It follows patterns
//! used in the midi_msg library crate and uses some types from it.
//!

use midi_msg::ParseError;
pub use midi_msg::{DeviceID, ManufacturerID};

/// Behringer's MIDI manufacturer ID.
pub const BEHRINGER: ManufacturerID = ManufacturerID(0x20u8, Some(0x32u8));

/// B-Control mode system exclusive data. All system exclusive message data
/// to or from the BC devices have this structure.
#[derive(Debug, PartialEq, Eq)]
pub struct BControlSysEx {
    pub device: DeviceID,
    pub model: BControlModel,
    pub command: BControlCommand,
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
            DeviceID::AllCall => 0x7f,
        });
        v.push(match self.model {
            BControlModel::BCR => 0x15,
            BControlModel::BCF => 0x14,
            BControlModel::Any => 0x7f,
        });
        self.command.extend_midi(v);
    }
    pub fn from_midi(m: &[u8]) -> Result<(Self, usize), ParseError> {
        if m.len() >= 3 {
            let device = match m[0] {
                0..=15 => DeviceID::Device(m[0]),
                0x7f => DeviceID::AllCall,
                n => return Err(ParseError::Invalid(format!("Invalid device id. ({n})"))),
            };
            let model = match m[1] {
                0x14 => BControlModel::BCF,
                0x15 => BControlModel::BCR,
                0x7f => BControlModel::Any,
                n => {
                    return Err(ParseError::Invalid(format!(
                        "Bad B-Control model number. ({n:x})"
                    )))
                }
            };
            let (command, used) = match m[2] {
                0x01 => (BControlCommand::RequestIdentity, 0),
                0x02 => (
                    BControlCommand::SendIdentity {
                        id_string: string_from_midi(&m[3..])?,
                    },
                    m.len()-3,
                ),
                0x20 => (
                    BControlCommand::SendBclMessage {
                        text: string_from_midi(&m[3..])?,
                    },
                    m.len()-3,
                ),
                0x21 => {
                    if m.len() > 6 {
                        // Supposedly the preset name will be exactly 26 chars.
                        (
                            BControlCommand::SendPresetName {
                                preset: PresetIndex::from_midi(&m[3..])?,
                                name: string_from_midi(&m[4..])?,
                            },
                            m.len()-3,
                        )
                    } else {
                        (
                            BControlCommand::BclReply {
                                msg_indx: u14_from_midi_msb_lsb(&m[3..])?,
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
                    m.len()-3,
                ),
                0x35 => (
                    BControlCommand::FirmwareReply {
                        mem_addr: u14_from_midi_msb_lsb(&m[3..])?,
                        err: u8_from_midi(&m[5..])?,
                    },
                    3,
                ),
                0x40 => (
                    BControlCommand::RequestData {
                        preset: PresetIndex::from_midi(&m[3..])?,
                    },
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
                cmd => {
                    return Err(ParseError::Invalid(format!(
                        "Invalid B-Control command {cmd:x}"
                    )))
                }
            };
            let result = BControlSysEx {
                device,
                model,
                command,
            };
            Ok((result, used + 3))
        } else {
            Err(ParseError::UnexpectedEnd)
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

/// B-Control command data appears in system exclusive messages sent to or
/// recieved from  B-Control devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BControlCommand {
    ///
    SendBclMessage {
        text: String,
    },

    RequestIdentity,
    SelectPreset {
        index: u8,
    },
    SendFirmware {
        data: Vec<u8>,
    },
    RequestData {
        preset: PresetIndex,
    },
    RequestGlobalSetup,
    RequestPresetName {
        preset: PresetIndex,
    },
    RequestSnapshot,

    SendIdentity {
        id_string: String,
    },
    BclReply {
        msg_indx: u16,
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
            BControlCommand::SendBclMessage { text } => {
                v.push(0x02);
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
            BControlCommand::RequestData { preset } => {
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
                msg_indx,
                error_code,
            } => {
                v.push(0x21);
                u14_to_midi_msb_lsb(*msg_indx, v);
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
    Preset { index: u8 },
    All,
    Temporary,
}

impl PresetIndex {
    fn from_midi(m: &[u8]) -> Result<PresetIndex, ParseError> {
        match u8_from_midi(m)? {
            0..=31 => Ok(PresetIndex::Preset { index: m[0] }),
            0x7e => Ok(PresetIndex::All),
            0x7f => Ok(PresetIndex::Temporary),
            n => return Err(ParseError::Invalid(format!("Bad preset index. ({n})"))),
        }
    }
    fn extend_midi(self, v: &mut Vec<u8>) {
        match self {
            PresetIndex::Preset { index } => v.push(index),
            PresetIndex::All => v.push(0x7e),
            PresetIndex::Temporary => v.push(0x7f),
        }
    }
}
#[inline]
fn u8_from_midi(m: &[u8]) -> Result<u8, ParseError> {
    if m.is_empty() {
        Err(ParseError::UnexpectedEnd)
    } else if m[0] > 127 {
        Err(ParseError::ByteOverflow)
    } else {
        Ok(m[0])
    }
}
#[inline]
fn u14_from_midi_msb_lsb(m: &[u8]) -> Result<u16, ParseError> {
    if m.len() < 2 {
        Err(ParseError::UnexpectedEnd)
    } else {
        let (msb, lsb) = (m[0], m[1]);
        if lsb > 127 || msb > 127 {
            Err(ParseError::ByteOverflow)
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
        Err(e) => Err(ParseError::Invalid(e.to_string())),
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
