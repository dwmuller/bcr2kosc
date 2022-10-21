#![allow(unused)]

use crate::b_control::BControlModel;

pub struct BclBlock {
    pub model: BControlModel,
    pub rev: Option<u8>,
    pub sections: Vec<BclSection>,
}

pub enum BclSection {
    Global(GlobalData),
    Preset,
    Button,
    Encoder,
    Fader
}

pub struct GlobalData {
    pub midimode: Option<MidiMode>,
    pub startup: Option<u8>,
    pub footsw: Option<()>,
    pub rxch: Option<()>,
    pub device_id: Option<()>,
    pub txinterval: Option<()>,
    pub deadtime: Option<()>
}

impl BclBlock {
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        s.push_str("$rev");
        match self.model {
            BControlModel::BCR => s += "R",
            BControlModel::BCF => s += "F",
        }
        if let Some(r) = self.rev {s += &r.to_string()};
        s .push('\n');
        s.push_str("$end\n");
        s
    }
}

pub enum MidiMode {
    U1, U2, U3, U4, S1, S2, S3, S4
}