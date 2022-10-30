//! MIDI/OSC translator for Behringer BCR2000
//!
//! An OSC server receives OSC packets at a configured UDP port and translates
//! them to MIDI/BCL messages sent to a BCR2000.
//!
//! An OSC client listens for MIDI/BCL messages from a BCR2000, translates them
//! to OSC packets, and sends them to a configured UDP port.

use crate::midi_util::find_port;
use crate::translator::{midi_to_osc, osc_to_midi};
use crate::PGM;
use log::{debug, error, info, warn};
use midi_control::MidiMessage;
use midir::{MidiInput, MidiInputPort, MidiOutput};
use rosc::encoder::encode;
use rosc::OscPacket;
use std::error::Error;
use std::marker::Unpin;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::{pin, select, spawn};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

type StopMechanism = Arc<Notify>;
pub struct BCtlOscSvc {
    pub midi_in_port_name: String,
    pub midi_out_port_name: String,
    pub osc_in_addr: SocketAddr,
    pub osc_out_addrs: Arc<Vec<SocketAddr>>,

    stopper: StopMechanism,
    spawned_tasks: Vec<JoinHandle<()>>,
}
impl BCtlOscSvc {
    pub fn new(
        midi_in_port_name: &str,
        midi_out_port_name: &str,
        osc_in_addr: &SocketAddr,
        osc_out_addrs: &[SocketAddr],
    ) -> Self {
        BCtlOscSvc {
            midi_in_port_name: midi_in_port_name.to_string(),
            midi_out_port_name: midi_out_port_name.to_string(),
            osc_in_addr: osc_in_addr.clone(),
            osc_out_addrs: Arc::new(osc_out_addrs.to_vec()),
            stopper: Arc::new(Notify::new()),
            spawned_tasks: Vec::new(),
        }
    }

    pub async fn start(&mut self) -> Result<(), Box<dyn Error>> {
        // We use a single UDP socket for sending and receiving.
        let udp_socket = Arc::new(UdpSocket::bind(self.osc_in_addr).await?);

        // MIDI -> OSC
        // Channel to receive MIDI messages.
        let (midi_tx, midi_rx) = mpsc::unbounded_channel();
        self.spawned_tasks.push(self.start_midi_listener(midi_tx)?);
        self.spawned_tasks
            .push(self.start_osc_sender(midi_rx, &udp_socket));

        // OSC -> MIDI
        // Channel to receive OSC messages.
        let (osc_tx, osc_rx) = mpsc::unbounded_channel();
        self.spawned_tasks
            .push(self.start_osc_listener(&udp_socket, osc_tx));
        self.spawned_tasks.push(self.start_midi_sender(osc_rx)?);

        Ok(())
    }

    pub async fn stop(&mut self) {
        self.stopper.notify_waiters();
        for ele in self.spawned_tasks.drain(0..) {
            // Seems to pend if the task completed. Might have to live with
            // .cancel instead if this becomes a problem.
            ele.await
                .or_else(|e| {
                    error!("Error waiting for tasks to stop: {e}");
                    Err(e)
                })
                .ok();
        }
    }

    fn start_midi_listener(
        &self,
        sender: mpsc::UnboundedSender<MidiMessage>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        let stopper = self.stopper.clone();
        let midi_input = MidiInput::new(&format!("{PGM} listening to B-Control"))?;
        let midi_input_port = find_port(&midi_input, &self.midi_in_port_name)?;
        Ok(spawn(async move {
            run_midi_listener(stopper, midi_input, midi_input_port, sender).await;
        }))
    }

    fn start_osc_sender(
        &self,
        receiver: mpsc::UnboundedReceiver<MidiMessage>,
        udp_socket: &Arc<UdpSocket>,
    ) -> JoinHandle<()> {
        let receiver = midi_to_osc(UnboundedReceiverStream::from(receiver));
        let osc_out_addrs = self.osc_out_addrs.clone();
        let udp_socket = udp_socket.clone();
        let value = spawn(async move {
            run_osc_sender(receiver, osc_out_addrs, udp_socket).await;
        });
        value
    }

    fn start_osc_listener(
        &self,
        udp_socket: &Arc<UdpSocket>,
        sender: mpsc::UnboundedSender<OscPacket>,
    ) -> JoinHandle<()> {
        let stopper = self.stopper.clone();
        let udp_socket = udp_socket.clone();
        spawn(async move {
            run_osc_listener(stopper, udp_socket, sender).await;
        })
    }

    fn start_midi_sender(
        &self,
        receiver: mpsc::UnboundedReceiver<OscPacket>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        let input = osc_to_midi(UnboundedReceiverStream::new(receiver));
        let input = Box::pin(input);
        let midi_output = MidiOutput::new(&format!("{PGM} feedback to B-Control"))?;
        let midi_output_port = find_port(&midi_output, &self.midi_out_port_name)?;
        info!(
            "{PGM} will send MIDI to {}.",
            midi_output.port_name(&midi_output_port).unwrap()
        );
        let midi_output_cxn = midi_output
            .connect(&midi_output_port, &format!("{PGM} sender"))
            .expect("Failed to open MIDI output connection.");
        Ok(spawn(async move {
            run_midi_sender(input, midi_output_cxn).await;
        }))
    }
}

async fn wait_on_stopping(stopper: StopMechanism) {
    stopper.notified().await;
}

async fn run_midi_listener(
    stopper: StopMechanism,
    midi_input: MidiInput,
    midi_input_port: MidiInputPort,
    out: mpsc::UnboundedSender<MidiMessage>,
) {
    info!(
        "{PGM} listening for MIDI on {}",
        midi_input.port_name(&midi_input_port).unwrap()
    );
    let midi_cxn = midi_input
        .connect(
            &midi_input_port,
            &format!("{PGM} listener"),
            move |_time, buff, _context| {midi_listener_callback(buff, &out);},
            (),
        )
        .expect("MIDI input port should have allowed a connection");
    wait_on_stopping(stopper).await;
    midi_cxn.close();
    info!("{PGM} MIDI listener stopped.")
}

fn midi_listener_callback(buff: &[u8], out: &mpsc::UnboundedSender<MidiMessage>) {
    let midi = MidiMessage::from(buff);
    if let MidiMessage::Invalid = midi {
        warn!("Invalid MIDI input, {} bytes.", buff.len());
    } else {
        debug!("Received MIDI msg: {midi:?}");
        out.send(midi)
            .or_else(|e| {
                error!("Failed to enqueue MIDI message: {e}");
                Err(e)
            })
            .ok();
    }
}

async fn run_osc_listener(
    stopper: StopMechanism,
    input: Arc<UdpSocket>,
    output: mpsc::UnboundedSender<OscPacket>,
) {
    info!(
        "{PGM} listening for OSC on UDP port {:?}.",
        input.local_addr()
    );
    let mut vec = vec![0u8; 1024 * 16];
    let mut next: usize = 0;
    let mut stop = false;
    while !stop {
        let stopper = stopper.clone();
        stop = select! {
            _ = recv_osc(&input, &mut vec, &mut next, &output) => {false},
            _ = wait_on_stopping(stopper) => {true}
        };
    }
    info!("{PGM} OSC listener stopped.");
}

async fn recv_osc(
    input: &Arc<UdpSocket>,
    vec: &mut Vec<u8>,
    next: &mut usize,
    output: &mpsc::UnboundedSender<OscPacket>,
) {
    match input.recv_from(&mut vec[*next..]).await {
        Ok((len, sender)) => {
            let buflen = *next + len;
            match rosc::decoder::decode_udp(&vec[0..buflen]) {
                Ok((remainder, pkt)) => {
                    debug!("Received OSC packet from {sender:?}: {pkt:?}");
                    let rlen = remainder.len();
                    if rlen > 0 {
                        debug!("OSC input remainder {len} bytes.");
                        vec.copy_within(len..len + rlen, 0);
                        *next = rlen;
                    }
                    output
                        .send(pkt)
                        .or_else(|e| {
                            error!("OSC input pkt dropped: {e}");
                            Err(e)
                        })
                        .ok();
                }
                Err(e) => {
                    error!("OSC pkt decode error: {e}");
                    *next = 0;
                    error!("Discarded {buflen} bytes.");
                }
            }
        }
        Err(e) => error!("UDP recv error: {e}"),
    }
}

async fn run_osc_sender<SRC>(src: SRC, osc_out_addrs: Arc<Vec<SocketAddr>>, dest: Arc<UdpSocket>)
where
    SRC: Stream<Item = OscPacket> + Unpin,
{
    info!("{PGM} will send OSC from UDP port {:?}.", dest.local_addr());
    pin!(src);
    while let Some(pkt) = src.next().await {
        let e = encode(&pkt);
        match e {
            Ok(buf) => {
                debug!("Sending this OSC packet: {pkt:?}");
                for a in &*osc_out_addrs {
                    if let Err(e) = dest.send_to(&buf, a).await {
                        error!("OSC send to {a} failed: {e}");
                    };
                }
            }
            Err(e) => error!("OSC encoding failed: {e}"),
        }
    }
    info!("{PGM} OSC sender stopped.");
}

async fn run_midi_sender<SRC: Stream<Item = MidiMessage> + Unpin>(
    mut input: SRC,
    mut midi_output_cxn: midir::MidiOutputConnection,
) {
    while let Some(midi) = input.next().await {
        debug!("Sending MIDI msg: {midi:?}");
        let bytes: Vec<u8> = midi.into();
        midi_output_cxn
            .send(&bytes)
            .or_else(|e| {
                error!("MIDI port send failed on {} bytes.", bytes.len());
                Err(e)
            })
            .ok();
    }
    info!("{PGM} MIDI sender stopped.")
}
