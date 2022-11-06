//! A module to create and control MIDI Stream and Sync structs that communicate
//! over streams in terms of data types defined by the `midi-control` crate. For
//! internal implementation, it relies on the platform-agnostic `midir` crate.
//! This module is runtime-agnostic, and is a good candidate for a distinct crate.

use std::error::Error;
use std::fmt::Display;
use std::pin::Pin;
use std::task::Poll;

use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::{self, UnboundedSender};
use futures::{Sink, Stream};
use log::{debug, error, info};
use midi_control::MidiMessage;
use midir::{MidiIO, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use pin_project::pin_project;

#[derive(Debug)]
pub enum MidiIoError {
    ChannelSender(mpsc::SendError),
    StdChannelSender(std::sync::mpsc::SendError<MidiMessage>),
    MidiInit(midir::InitError),
    MidiSend(midir::SendError),
    MidiInputConnect(midir::ConnectError<MidiInput>),
    SpawnError(futures::task::SpawnError),
    Regular(ErrorKind),
}

#[derive(Clone, Copy, Debug)]
pub enum ErrorKind {
    MidiPortNameNotFound,
    NotConnected,
}
impl Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            ErrorKind::MidiPortNameNotFound => "named MIDI port not found",
            ErrorKind::NotConnected => "not connected to a MIDI port",
        }.fmt(f)
    }
}

impl Display for MidiIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiIoError::ChannelSender(e) => e.fmt(f),
            MidiIoError::StdChannelSender(e) => e.fmt(f),
            MidiIoError::MidiInit(e) => e.fmt(f),
            MidiIoError::MidiSend(e) => e.fmt(f),
            MidiIoError::MidiInputConnect(e) => e.fmt(f),
            MidiIoError::SpawnError(e) => e.fmt(f),
            MidiIoError::Regular(k) => k.fmt(f),
        }
    }
}

impl Error for MidiIoError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }

    fn cause(&self) -> Option<&dyn Error> {
        self.source()
    }
}

impl From<ErrorKind> for MidiIoError {
    fn from(value: ErrorKind) -> Self {
        MidiIoError::Regular(value)
    }
}
impl From<mpsc::SendError> for MidiIoError {
    fn from(e: mpsc::SendError) -> Self {
        MidiIoError::ChannelSender(e)
    }
}
impl From<std::sync::mpsc::SendError<MidiMessage>> for MidiIoError {
    fn from(e: std::sync::mpsc::SendError<MidiMessage>) -> Self {
        MidiIoError::StdChannelSender(e)
    }
}
impl From<midir::InitError> for MidiIoError {
    fn from(e: midir::InitError) -> Self {
        MidiIoError::MidiInit(e)
    }
}

impl From<midir::SendError> for MidiIoError {
    fn from(e: midir::SendError) -> Self {
        MidiIoError::MidiSend(e)
    }
}

impl From<midir::ConnectError<MidiInput>> for MidiIoError {
    fn from(e: midir::ConnectError<MidiInput>) -> Self {
        MidiIoError::MidiInputConnect(e)
    }
}
impl From<futures::task::SpawnError> for MidiIoError {
    fn from(e: futures::task::SpawnError) -> Self {
        MidiIoError::SpawnError(e)
    }
}
pub type Result<T> = std::result::Result<T, MidiIoError>;

pub fn find_port<T: MidiIO>(midi_io: &T, port_name: &str) -> Result<T::Port> {
    let ports = midi_io.ports();
    let wanted = Ok(port_name.to_string());
    let port = ports.iter().find(|&x| midi_io.port_name(&x) == wanted);
    match port {
        Some(p) => Ok(p.clone()),
        None => Err(MidiIoError::Regular(ErrorKind::MidiPortNameNotFound)),
    }
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
            debug!("midi-io sent MIDI msg");
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
