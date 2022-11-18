#![deny(missing_docs)]
//! Asynchronous MIDI I/O via `Stream` and `Sink`.
//! 
//! A module that wraps MIDI port I/O mechanisms with MIDI `Stream` and `Sink`
//! structs that work with types from the `midi-control` crate. For internal
//! implementation, it relies on the platform-agnostic `midir` crate.
//! 
//! This module is runtime-agnostic, and is a good candidate for a distinct crate.

use std::pin::Pin;
use std::task::Poll;

use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::{Sink, Stream};
use log::{debug, error, info};
use midi_control::MidiMessage;
use midir::{MidiIO, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use pin_project::pin_project;

mod error;
pub use error::*;

/// Provides a snapshot of input port names. This list can differ on
/// subsequent calls, as MIDI devices are connected or disconnected.
pub fn input_ports() -> Vec<String> {
    let midi_in = MidiInput::new("{PGM} list_ports").unwrap();
    midi_in
        .ports()
        .iter()
        .map(|p| midi_in.port_name(p).unwrap())
        .collect()
}

/// Provides a snapshot of input port names. This list can differ on
/// subsequent calls, as MIDI devices are connected or disconnected.
pub fn output_ports() -> Vec<String> {
    let midi_out = MidiOutput::new("{PGM} list_ports").unwrap();
    midi_out
        .ports()
        .iter()
        .map(|p| midi_out.port_name(p).unwrap())
        .collect()
}
/// A stream that provides MIDI messages recieved from a named MIDI I/O port.
/// The stream is backed by an unbounded channel. The connection to the port is
/// closed when the stream is dropped.
pub struct MidiStream {
    /// Keep this alive until we stop. Since `midir` is callback-driven, we
    /// don't actually need to reference this once it's set up.
    _midi_cxn: MidiInputConnection<()>,

    /// Our underlying stream implementation. The callback can run at an time,
    /// so we need this buffered storage for it. The callback is also
    /// synchronous,so we need the unbounded channel's ability to receive data
    /// synchronously.
    rx: UnboundedReceiver<MidiMessage>,
}

impl Stream for MidiStream {
    type Item = MidiMessage;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let rx = Pin::new(&mut self.get_mut().rx);
        rx.poll_next(cx)
    }
}

impl MidiStream {
    /// Creates a new MidiListener stream for the named MIDI I/O port.
    pub fn bind(port_name: &str) -> Result<MidiStream> {
        let midi_input = MidiInput::new(&format!("midi-io MIDI input"))?;
        let midi_input_port = find_port(&midi_input, port_name)?;
        let (tx, rx) = mpsc::unbounded();

        let cb = move |_time: u64, buf: &[u8], _context: &mut ()| {
            debug!("midi-io received {} bytes.", buf.len());
            let midi = MidiMessage::from(buf);
            tx.unbounded_send(midi)
                .or_else(|e| {
                    error!("midi-io listener error on send: {e}");
                    Err(e)
                })
                .ok();
        };
        let midi_cxn = midi_input.connect(&midi_input_port, "midi-io listener", cb, ())?;
        info!("midi-io listener started on \"{port_name}\"");

        Ok(MidiStream {
            rx,
            _midi_cxn: midi_cxn,
        })
    }
}

/// A Sink which transmits MIDI messages in the form of
/// `midi_connect::MidiMessage` structs to a single MIDI port.
#[pin_project]
pub struct MidiSink {
    #[pin]
    data_q: Option<std::sync::mpsc::Sender<MidiMessage>>,
    #[pin]
    response_q: mpsc::UnboundedReceiver<bool>,
    pending_count: usize,
}

// Windows MIDI port drivers may or may not pend when sending. This
// implementation assumes that they do, and output is performed on a separate task. In order to verify that a send is
// complete (at least to the point of handoff to the API), we use a response
// channel

impl MidiSink {
    /// Returns a new `MidiSink` bound to the named MIDI port.
    /// 
    /// This starts an OS thread to handle writes, which may be synchronous,
    /// depending on operating system and MIDI port driver.
    pub fn bind(port_name: &str) -> Result<Self> {
        let midi_output = MidiOutput::new(&format!("midi-io MIDI output"))?;
        let midi_output_port = find_port(&midi_output, port_name)?;
        let midi_cxn = midi_output
            .connect(&midi_output_port, &format!("midi-io sender"))
            .expect("Failed to open MIDI output connection.");
        let (data_tx, data_rx) = std::sync::mpsc::channel::<MidiMessage>();
        let (response_tx, response_rx) = mpsc::unbounded::<bool>();
        let port_name = port_name.to_string();
        info!("midi-io writer started on \"{port_name:}\"");
        std::thread::spawn(|| {
            run_midi_writer(data_rx, midi_cxn, response_tx);
        });
        Ok(MidiSink {
            data_q: Some(data_tx),
            response_q: response_rx,
            pending_count: 0,
        })
    }
}

fn run_midi_writer(
    data_rx: std::sync::mpsc::Receiver<MidiMessage>,
    mut midi_cxn: MidiOutputConnection,
    response_tx: UnboundedSender<bool>,
) {
    // The only significant recv error is due to channel closure.
    while let Ok(item) = data_rx.recv() {
        debug!("midi-io sending MIDI msg: {item:?}");
        let bytes: Vec<u8> = item.into();
        let result = midi_cxn.send(&bytes).map_err(MidiIoError::from);
        if let Err(e) = result {
            error!("midi-io send error: {e:?}");
        } else {
            debug!("midi-io sent {} bytes.", bytes.len());
        }
        if let Err(e) = response_tx.unbounded_send(true) {
            error!("midi-io response send error: {e}");
        }
    }
    info!("midi-io listener thread exiting")
}

impl Sink<MidiMessage> for MidiSink {
    type Error = MidiIoError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<()>> {
        self.poll_flush(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: MidiMessage) -> Result<()> {
        match self.data_q {
            Some(ref data_q) => data_q.send(item).map_err(MidiIoError::from).and_then(|v| {
                *self.project().pending_count += 1;
                Ok(v)
            }),
            None => Err(MidiIoError::from(ErrorKind::NotConnected)),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<()>> {
        while *self.as_mut().project().pending_count > 0 {
            let this = self.as_mut().project();
            if let Poll::Ready(Some(_)) = this.response_q.poll_next(cx) {
                *this.pending_count -= 1;
            } else {
                return Poll::Pending;
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Result<()>> {
        if let Poll::Ready(Ok(())) = self.as_mut().poll_flush(cx) {
            self.data_q = None;
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}


fn find_port<T: MidiIO>(midi_io: &T, port_name: &str) -> Result<T::Port> {
    let ports = midi_io.ports();
    let wanted = Ok(port_name.to_string());
    let port = ports.iter().find(|&x| midi_io.port_name(&x) == wanted);
    match port {
        Some(p) => Ok(p.clone()),
        None => Err(MidiIoError::Regular(ErrorKind::MidiPortNameNotFound)),
    }
}

