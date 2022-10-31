//! A module to create and control MIDI sender and lister tasks that communicate
//! over streams in terms of data types defined by the `midi-control` crate. For
//! internal implementation, it relies on the platform-agnostic `midir crate`.
//!

use std::error::Error;
use std::pin::Pin;

use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::Stream;
use log::error;
use midi_control::MidiMessage;
use midir::{MidiInput, MidiInputConnection};

use crate::midi_util::find_port;

/// A stream that provides MIDI messages recieved from a named MIDI I/O port.
/// The stream is backed by an unbounded channel. The connection to the port is
/// closed when the stream is dropped.
pub struct MidiListenerStream {
    /// Our underlying stream implementation.
    rx: UnboundedReceiver<MidiMessage>,
    /// Keep this alive until we've been dropped. When the connection is
    /// dropped/closed, the callbacks will stop.
    _cxn: MidiInputConnection<()>,
}

impl Stream for MidiListenerStream {
    type Item = MidiMessage;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let rx = Pin::new(&mut self.get_mut().rx);
        rx.poll_next(cx)
    }
}

impl MidiListenerStream {
    /// Creates a new MidiListener stream for the named MIDI I/O port. 
    pub fn new(port_name: &str) -> Result<impl Stream<Item = MidiMessage>, Box<dyn Error>> {
        let midi_input = MidiInput::new(&format!("midi-io listener"))?;
        let midi_input_port = find_port(&midi_input, port_name)?;
        let (tx, rx) = mpsc::unbounded();

        let cb = move |_time: u64, buf: &[u8], _context: &mut ()| {
            let midi = MidiMessage::from(buf);
            tx.unbounded_send(midi)
                .or_else(|e| {
                    error!("midi-io listener error on send: {e}");
                    Err(e)
                })
                .ok();
        };

        let _cxn = midi_input.connect(&midi_input_port, "midi-io listener", cb, ())?;

        Ok(MidiListenerStream { rx, _cxn })
    }
}
