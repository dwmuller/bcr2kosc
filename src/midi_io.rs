//! A module to create and control MIDI Stream and Sync structs that communicate
//! over streams in terms of data types defined by the `midi-control` crate. For
//! internal implementation, it relies on the platform-agnostic `midir` crate.
//! This module is runtime-agnostic, and is a good candidate for a distinct crate.

use std::error::Error;
use std::io::ErrorKind;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::{ready, FutureExt, Sink, Stream};
use log::{debug, error, info};
use midi_control::MidiMessage;
use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use pin_project_lite::pin_project;

use crate::midi_util::find_port;

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
    pub fn bind(port_name: &str) -> Result<impl Stream<Item = MidiMessage>, Box<dyn Error>> {
        let midi_input = MidiInput::new(&format!("midi-io MIDI input"))?;
        let midi_input_port = find_port(&midi_input, port_name)?;
        let (tx, rx) = mpsc::unbounded();

        let cb = move |_time: u64, buf: &[u8], _context: &mut ()| {
            let midi = MidiMessage::from(buf);
            debug!("Received MIDI msg: {midi:?}");
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

pin_project! {
    #[must_use = "sinks do nothing unless polled"]
    /// A Sink which transmits MIDI messages in the form of
    /// `midi_connect::MidiMessage` structs to a single MIDI port.
    pub struct MidiSink<'a>
    {
        midi_cxn: Option<Arc<Mutex<MidiOutputConnection>>>,
        #[pin]
        future: Option<BoxFuture<'a, Result<(),midir::SendError>>>,
        // future: Option<Box<dyn Future<Output=Result<(),midir::SendError>>>>,
    }

}

impl<'a> MidiSink<'a> {
    /// Returns a new `MidiSink` bound to the named MIDI port.
    pub fn bind(port_name: &str) -> Result<Self, Box<dyn Error>> {
        let midi_output = MidiOutput::new(&format!("midi-io MIDI output"))?;
        let midi_output_port = find_port(&midi_output, port_name)?;
        let midi_cxn = midi_output
            .connect(&midi_output_port, &format!("midi-io sender"))
            .expect("Failed to open MIDI output connection.");
        let midi_cxn = Some(Arc::new(Mutex::new(midi_cxn)));
        Ok(Self {
            midi_cxn,
            future: None,
        })
    }
}

impl<'a> Sink<MidiMessage> for MidiSink<'a> {
    type Error = std::io::Error;

    fn poll_ready(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: MidiMessage) -> Result<(), Self::Error> {
        if self.future.is_some() {
            panic!("start_send called without poll_ready being called first");
        }
        let mut this = self.project();
        if let Some(midi_cxn) = this.midi_cxn {
            debug!("Sending MIDI msg: {item:?}");
            let bytes: Vec<u8> = item.into();
            let midi_cxn = midi_cxn.clone();
            let f = async move { midi_cxn.lock_owned().await.send(&bytes) };
            this.future.set(Some(f.boxed()));
            Ok(())
        } else {
            Err(Self::Error::from(ErrorKind::NotConnected))
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        let mut this = self.project();
        Poll::Ready(if this.midi_cxn.is_none() {
            Err(Self::Error::from(ErrorKind::NotConnected))
        } else if this.future.is_none() {
            Ok(())
        } else {
            let mut f = this.future.take().unwrap();
            match ready!(f.poll_unpin(cx)) {
                Ok(val) => Ok(val),
                Err(e) => Err(Self::Error::new(ErrorKind::Other, e)),
            }
        })
    }

    fn poll_close(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

/// Provides a snapshot of input port names. This list can differ on
/// subsequent calls, as MIDI devices are connected or disconnected.
pub fn input_ports() -> Vec<String> {
    todo!()
}

/// Provides a snapshot of input port names. This list can differ on
/// subsequent calls, as MIDI devices are connected or disconnected.
pub fn output_ports() -> Vec<String> {
    todo!()
}
