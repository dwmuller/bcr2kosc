//! Error definitions for `midi-io`.
//! 
use std::error::Error;
use std::fmt::Display;

use futures::channel::mpsc;
use midi_msg::MidiMsg;
use midir::MidiInput;


/// Error enum for errors originating in or evoked by `midi-io`.
#[derive(Debug)]
pub enum MidiIoError {
    ChannelSender(mpsc::SendError),
    StdChannelSender(std::sync::mpsc::SendError<MidiMsg>),
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
impl From<std::sync::mpsc::SendError<MidiMsg>> for MidiIoError {
    fn from(e: std::sync::mpsc::SendError<MidiMsg>) -> Self {
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
