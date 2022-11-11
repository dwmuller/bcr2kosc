//! Easy I/O of B-Control messages via `MidiMsg` `Stream` and `Sink`.

use std::error::Error;

use futures::{Sink, SinkExt, Stream, StreamExt};
use midi_msg::MidiMsg;

use super::{BControlCommand, BControlModel, BControlSysEx, DeviceID, PresetIndex};

type LocalError = Box<dyn Error + Send + Sync + 'static>;
type Result<T> = std::result::Result<T, LocalError>;

pub async fn recv_bcl<I>(device: u8, midi_in: &mut I) -> Result<Vec<String>>
where
    I: Stream<Item = MidiMsg> + Unpin,
{
    let mut v = Vec::<String>::new();
    let mut next_line_index = 0;
    while let Some(msg) = midi_in.next().await {
        if let Some(sysex) = BControlSysEx::try_from(&msg).ok() {
            if sysex.device.match_device(device) {
                if let BControlCommand::SendBclMessage { msg_index, text } = sysex.command {
                    if msg_index == next_line_index {
                        next_line_index += 1;
                        let done = text == "$end";
                        v.push(text);
                        if done {
                            break;
                        }
                    } else {
                        return Err(LocalError::from(
                            "Missing or out-of-order BCL lines received.",
                        ));
                    }
                }
            }
        }
    }
    Ok(v)
}

pub async fn get_preset_bcl<I, O>(
    device: u8,
    preset: PresetIndex,
    midi_in: &mut I,
    midi_out: &mut O,
) -> Result<Vec<String>>
where
    I: Stream<Item = MidiMsg> + Unpin,
    O: Sink<MidiMsg> + Unpin,
    O::Error: std::error::Error + Send + Sync + 'static,
{
    let lines = recv_bcl(device, midi_in);

    let bdata = BControlSysEx {
        device: DeviceID::Device(device),
        model: BControlModel::Any,
        command: BControlCommand::RequestData(preset),
    };
    midi_out
        .send(MidiMsg::from(&bdata))
        .await
        .map_err(|e| LocalError::from(e))?;
    lines.await
}

pub async fn get_global_bcl<I, O>(
    device: u8,
    midi_in: &mut I,
    midi_out: &mut O,
) -> Result<Vec<String>>
where
    I: Stream<Item = MidiMsg> + Unpin,
    O: Sink<MidiMsg> + Unpin,
    O::Error: std::error::Error + Send + Sync + 'static,
{
    let lines = recv_bcl(device, midi_in);

    let bdata = BControlSysEx {
        device: DeviceID::Device(device),
        model: BControlModel::Any,
        command: BControlCommand::RequestGlobalSetup,
    };
    midi_out
        .send(MidiMsg::from(&bdata))
        .await
        .map_err(|e| LocalError::from(e))?;
    lines.await
}
